//! Claude Code 会话日志使用追踪
//!
//! 从 ~/.claude/projects/ 下的 JSONL 会话文件中提取 token 使用数据，
//! 实现无代理模式下的使用统计。
//!
//! ## 数据流
//! ```text
//! ~/.claude/projects/*/*.jsonl → 增量解析 → 去重 → 费用计算 → proxy_request_logs 表
//! ```

use crate::config::get_claude_config_dir;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::{CostCalculator, ModelPricing};
use crate::proxy::usage::parser::TokenUsage;
use crate::services::usage_stats::{
    effective_usage_log_filter, find_model_pricing, should_skip_session_insert, DedupKey,
    SESSION_PROXY_DEDUP_WINDOW_SECONDS,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 同步结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSyncResult {
    pub imported: u32,
    pub skipped: u32,
    pub files_scanned: u32,
    pub errors: Vec<String>,
}

/// 数据来源分布
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceSummary {
    pub data_source: String,
    pub request_count: u32,
    pub total_cost_usd: String,
}

/// 从 JSONL 中解析出的 assistant 消息使用数据
#[derive(Debug)]
struct ParsedAssistantUsage {
    message_id: String,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    stop_reason: Option<String>,
    timestamp: Option<String>,
    session_id: Option<String>,
}

/// 同步 Claude Code 会话日志到使用统计数据库
pub fn sync_claude_session_logs(db: &Database) -> Result<SessionSyncResult, AppError> {
    let projects_dir = get_claude_config_dir().join("projects");
    if !projects_dir.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            errors: vec![],
        });
    }

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 0,
        errors: vec![],
    };

    // 收集所有 .jsonl 文件
    let jsonl_files = collect_jsonl_files(&projects_dir);

    for file_path in &jsonl_files {
        result.files_scanned += 1;

        match sync_single_file(db, file_path) {
            Ok((imported, skipped)) => {
                result.imported += imported;
                result.skipped += skipped;
            }
            Err(e) => {
                let msg = format!("{}: {e}", file_path.display());
                log::warn!("[SESSION-SYNC] 文件解析失败: {msg}");
                result.errors.push(msg);
            }
        }
    }

    if result.imported > 0 {
        log::info!(
            "[SESSION-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }

    // 会话 → 项目归因：按 ~/.claude/projects/<encoded-cwd> 回填 Claude 用量行的
    // project_id。归因是「尽力而为」的增强（不影响 token/成本数据），失败仅告警不阻断同步。
    match attribute_claude_sessions_to_projects(db) {
        Ok(n) if n > 0 => log::info!("[SESSION-SYNC] 归因 project_id 回填 {} 行", n),
        Ok(_) => {}
        Err(e) => log::warn!("[SESSION-SYNC] project_id 归因失败: {e}"),
    }

    Ok(result)
}

/// 镜像 Claude Code 的项目目录编码：把 `:`、`\`、`/`、`_` 替换为 `-`。
///
/// 例：`D:\work\slg` → `D--work-slg`；`D:\work\slg_google_beta` → `D--work-slg-google-beta`。
/// 采用「正向编码项目路径」而非「解码目录名」，规避 `-` ↔ {`\`,`/`,`_`} 的有损歧义。
fn encode_cwd_to_claude_dir(path: &str) -> String {
    path.chars()
        .map(|c| match c {
            ':' | '\\' | '/' | '_' => '-',
            _ => c,
        })
        .collect()
}

/// 同一 `message.id` 的多条 assistant 记录的去重判定（与 `sync_single_file` 同口径）：
/// 优先保留有 `stop_reason` 的；否则取 `output_tokens` 更大者。
fn should_replace_assistant(prev: &ParsedAssistantUsage, new: &ParsedAssistantUsage) -> bool {
    if new.stop_reason.is_some() && prev.stop_reason.is_none() {
        return true;
    }
    if new.stop_reason.is_some() == prev.stop_reason.is_some() {
        return new.output_tokens > prev.output_tokens;
    }
    false
}

/// 解析单个会话 `*.jsonl`，按 `message.id` 去重后返回有计费 token 的 assistant 消息。
///
/// 供工程归因使用：每条消息的 (model, tokens, timestamp) 指纹用于匹配代理行。
fn parse_assistant_messages_in_file(path: &Path) -> Vec<ParsedAssistantUsage> {
    let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();

    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    for line_result in BufReader::new(file).lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue, // 容忍不完整的最后一行
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let message = match value.get("message") {
            Some(m) => m,
            None => continue,
        };
        let msg_id = match message.get("id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let usage = match message.get("usage") {
            Some(u) => u,
            None => continue,
        };
        let parsed = ParsedAssistantUsage {
            message_id: msg_id.clone(),
            model: message
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input_tokens: usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            stop_reason: message
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            timestamp: value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            session_id: None,
        };
        let should_replace = match messages.get(&msg_id) {
            None => true,
            Some(existing) => should_replace_assistant(existing, &parsed),
        };
        if should_replace {
            messages.insert(msg_id, parsed);
        }
    }

    messages
        .into_values()
        .filter(|m| {
            m.input_tokens > 0
                || m.output_tokens > 0
                || m.cache_read_tokens > 0
                || m.cache_creation_tokens > 0
        })
        .collect()
}

/// 用消息指纹（model + 各 token 维度 + ±窗口时间戳）回填匹配 Claude 行的 `project_id`。
///
/// 指纹口径与跨源去重 `has_matching_proxy_usage_log` 一致，因此命中的就是该会话消息
/// 对应的代理行（代理行的 session_id 来自请求头，与会话文件名不一致，无法直接相连，
/// 只能靠指纹匹配）。不限定 `data_source`，故 session_log 行也会被一并归因；幂等。
/// 每次调用独立获取 DB 锁，避免长扫描期间长时间持锁阻塞其它 DB 操作。
fn update_project_id_by_fingerprint(
    db: &Database,
    project_id: &str,
    msg: &ParsedAssistantUsage,
) -> Result<usize, AppError> {
    let created_at = msg
        .timestamp
        .as_ref()
        .and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.timestamp())
        })
        .unwrap_or(0);
    if created_at == 0 {
        return Ok(0); // 无时间戳无法做时间窗口匹配
    }

    let conn = lock_conn!(db.conn);
    let updated = conn
        .execute(
            "UPDATE proxy_request_logs SET project_id = ?1
             WHERE project_id IS NULL AND app_type = 'claude'
               AND status_code >= 200 AND status_code < 300
               AND input_tokens = ?2 AND output_tokens = ?3
               AND cache_read_tokens = ?4 AND cache_creation_tokens = ?5
               AND created_at BETWEEN ?6 - ?7 AND ?6 + ?7
               AND (LOWER(model) = LOWER(?8) OR LOWER(model) = 'unknown' OR LOWER(?8) = 'unknown')",
            rusqlite::params![
                project_id,
                msg.input_tokens as i64,
                msg.output_tokens as i64,
                msg.cache_read_tokens as i64,
                msg.cache_creation_tokens as i64,
                created_at,
                SESSION_PROXY_DEDUP_WINDOW_SECONDS,
                msg.model,
            ],
        )
        .map_err(|e| AppError::Database(format!("回填 project_id 失败: {e}")))?;
    Ok(updated)
}

/// 把 Claude 会话归因到项目并回填 `project_id`（在每次会话同步末尾调用）。
///
/// 思路：Claude Code 会话文件位于 `~/.claude/projects/<encoded-cwd>/*.jsonl`，目录名
/// 即编码后的工作目录。把每个项目的 `path` 同样编码后与目录名匹配，命中的目录里解析
/// 出 assistant 消息，用指纹回填对应 Claude 用量行的 `project_id`。
///
/// 幂等：仅更新 `project_id IS NULL` 的行。
///
/// 增量：用 settings 表记录「上次扫描时间」与「上次的项目集合签名」。每次只解析
/// `mtime > 上次扫描时间` 的会话文件，避免在 steady-state（或大量来自未登记目录、
/// 永远无法归因的 Claude 行存在时）反复全量扫描。项目增删/改路径会使签名变化 →
/// 触发一次全量重扫，保证新项目的历史用量也被归因。
pub fn attribute_claude_sessions_to_projects(db: &Database) -> Result<usize, AppError> {
    const SETTING_SCAN_NS: &str = "claude_session_attr_last_scan_ns";
    const SETTING_PROJ_SIG: &str = "claude_session_attr_proj_sig";

    // encoded-cwd → project_id
    let mut by_encoded: HashMap<String, String> = db
        .list_projects(false)?
        .into_iter()
        .map(|p| (encode_cwd_to_claude_dir(&p.path), p.id))
        .collect();
    let current_sig = project_signature(&by_encoded);

    // 项目集合变化（新增/删除/改路径）→ 强制全量重扫；否则按上次扫描时间增量扫
    let sig_unchanged = db.get_setting(SETTING_PROJ_SIG)? == Some(current_sig.clone());
    let last_scan_ns = if sig_unchanged {
        db.get_setting(SETTING_SCAN_NS)?
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let scan_start_ns = now_nanos();

    if by_encoded.is_empty() {
        // 没有项目也要推进扫描水位，避免下次空过一遍目录
        db.set_setting(SETTING_SCAN_NS, &scan_start_ns.to_string())?;
        db.set_setting(SETTING_PROJ_SIG, &current_sig)?;
        return Ok(0);
    }

    let projects_dir = get_claude_config_dir().join("projects");
    let mut total_updated = 0usize;

    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return Ok(0), // 还没跑过 Claude Code
    };
    for entry in entries.flatten() {
        let dir_path = entry.path();
        if !dir_path.is_dir() {
            continue;
        }
        let dir_name = match dir_path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let project_id = match by_encoded.remove(dir_name) {
            Some(p) => p,
            None => continue, // 用户未登记此工作目录为项目
        };

        // 只解析自上次扫描后变更过的顶层 *.jsonl（增量）
        let files = match fs::read_dir(&dir_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for file_entry in files.flatten() {
            let file_path = file_entry.path();
            if !(file_path.is_file()
                && file_path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            {
                continue;
            }
            let mtime_ns = fs::metadata(&file_path)
                .ok()
                .map(|m| metadata_modified_nanos(&m))
                .unwrap_or(0);
            if mtime_ns <= last_scan_ns {
                continue;
            }
            for msg in parse_assistant_messages_in_file(&file_path) {
                total_updated += update_project_id_by_fingerprint(db, &project_id, &msg)?;
            }
        }
    }

    db.set_setting(SETTING_SCAN_NS, &scan_start_ns.to_string())?;
    db.set_setting(SETTING_PROJ_SIG, &current_sig)?;
    Ok(total_updated)
}

/// 当前项目集合的稳定签名（排序后的 encoded-cwd 列表）。增删项目或改路径都会变化。
fn project_signature(by_encoded: &HashMap<String, String>) -> String {
    let mut keys: Vec<&str> = by_encoded.keys().map(|s| s.as_str()).collect();
    keys.sort_unstable();
    keys.join(",")
}

/// 当前时间的 Unix 纳秒戳（`SystemTime::now` 封装，便于测试与一致性）。
fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// 收集目录下所有 .jsonl 文件（含子 agent 文件）
///
/// 扫描固定深度，不使用递归，避免死循环：
///   projects_dir/项目目录/*.jsonl                                      (主会话)
///   projects_dir/项目目录/SESSION_ID/subagents/*.jsonl                  (Task/Agent 子 agent)
///   projects_dir/项目目录/SESSION_ID/subagents/workflows/wf_*/*.jsonl   (Workflow 子 agent)
///
/// 最后一层是 Claude Code Workflow 功能产生的子 agent transcript，比普通子
/// agent 多嵌套一层 `workflows/wf_<ID>/`。漏掉这一层会让 Workflow 的 token
/// 用量完全不计入统计；`journal.jsonl` 不含 `type=="assistant"` 行，解析时
/// 会被 `sync_single_file` 天然跳过，因此这里无需按文件名过滤。
fn collect_jsonl_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let entries = match fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // 每个项目目录下的 .jsonl 文件
        if let Ok(sub_entries) = fs::read_dir(&path) {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    // 主会话 JSONL 文件
                    files.push(sub_path);
                } else if sub_path.is_dir() {
                    // 扫描子 agent 目录: 项目/SESSION_ID/subagents/*.jsonl
                    let subagents_dir = sub_path.join("subagents");
                    if subagents_dir.is_dir() {
                        push_jsonl_children(&subagents_dir, &mut files);

                        // 额外下探 Workflow 子 agent:
                        // 项目/SESSION_ID/subagents/workflows/wf_<ID>/*.jsonl
                        let workflows_dir = subagents_dir.join("workflows");
                        if workflows_dir.is_dir() {
                            if let Ok(wf_entries) = fs::read_dir(&workflows_dir) {
                                for wf_entry in wf_entries.flatten() {
                                    let wf_path = wf_entry.path();
                                    if wf_path.is_dir() {
                                        push_jsonl_children(&wf_path, &mut files);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    files
}

/// 将 `dir` 下直接子层的所有 `.jsonl` 文件追加到 `files`（不递归）。
fn push_jsonl_children(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
}

/// 同步单个 JSONL 文件，返回 (imported, skipped)
fn sync_single_file(db: &Database, file_path: &Path) -> Result<(u32, u32), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // 获取文件元数据
    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);

    // 检查同步状态
    let (last_modified, last_offset) = get_sync_state(db, &file_path_str)?;

    // 文件未变化则跳过
    if file_modified <= last_modified {
        return Ok((0, 0));
    }

    // 从上次偏移位置开始增量解析
    let file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let reader = BufReader::new(file);

    let mut line_offset: i64 = 0;
    let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();
    let mut current_session_id: Option<String> = None;

    for line_result in reader.lines() {
        line_offset += 1;

        // 跳过已处理的行
        if line_offset <= last_offset {
            continue;
        }

        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue, // 容忍不完整的最后一行
        };

        if line.trim().is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // 提取 session ID (从 system 或首条消息)
        if current_session_id.is_none() {
            if let Some(sid) = value.get("sessionId").and_then(|v| v.as_str()) {
                current_session_id = Some(sid.to_string());
            }
        }

        // 只处理 assistant 类型的消息
        if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }

        let message = match value.get("message") {
            Some(m) => m,
            None => continue,
        };

        let msg_id = match message.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let usage = match message.get("usage") {
            Some(u) => u,
            None => continue,
        };

        let parsed = ParsedAssistantUsage {
            message_id: msg_id.clone(),
            model: message
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input_tokens: usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            stop_reason: message
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            timestamp: value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            session_id: current_session_id.clone(),
        };

        // 按 message.id 去重：优先保留有 stop_reason 的条目，否则保留最新的
        let should_replace = match messages.get(&msg_id) {
            None => true,
            Some(existing) => {
                // 新条目有 stop_reason 而旧条目没有 → 替换
                if parsed.stop_reason.is_some() && existing.stop_reason.is_none() {
                    true
                }
                // 两个都有或都没有 stop_reason → 取 output_tokens 更大的
                else if parsed.stop_reason.is_some() == existing.stop_reason.is_some() {
                    parsed.output_tokens > existing.output_tokens
                } else {
                    false
                }
            }
        };

        if should_replace {
            messages.insert(msg_id, parsed);
        }
    }

    // 写入数据库
    let mut imported: u32 = 0;
    let mut skipped: u32 = 0;

    for msg in messages.values() {
        // 只要产生了真实计费 token 就导入，不再强制要求 stop_reason 或 output>0。
        //
        // Anthropic 在受理请求时即对 input + cache_read + cache_creation 计费
        // （这些在请求开始就确定），output 按实际生成量计。Workflow / 子 agent 的
        // 并行短命请求经常只写了 message_start 快照（output=1、stop_reason=None）
        // 却没有写最终块，但其 cache/input 成本已被真实计费。旧逻辑用 stop_reason
        // 非空 + output>0 双重过滤，会把这类请求整条丢弃，实测系统性低估约 4.1%，
        // 且 92% 集中在 workflow/subagent。这里改为「任一计费维度 > 0 即导入」。
        //
        // 去重选择逻辑（上方按 message.id 取 stop_reason 优先 / output 最大者）保持
        // 不变：它选出的代表行的 input/cache 本就准确；request_id = session:msg_id
        // 主键 + INSERT OR IGNORE 保证一个 message 仍只落库一次，放宽 gate 不会双算。
        let has_billable_tokens = msg.input_tokens > 0
            || msg.output_tokens > 0
            || msg.cache_read_tokens > 0
            || msg.cache_creation_tokens > 0;
        if !has_billable_tokens {
            continue;
        }

        let request_id = format!(
            "{}{}",
            crate::proxy::usage::parser::SESSION_REQUEST_ID_PREFIX,
            msg.message_id
        );

        match insert_session_log_entry(db, &request_id, msg) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                log::warn!("[SESSION-SYNC] 插入失败 ({}): {e}", msg.message_id);
                skipped += 1;
            }
        }
    }

    // 更新同步状态
    update_sync_state(db, &file_path_str, file_modified, line_offset)?;

    Ok((imported, skipped))
}

/// 获取 session_log_sync 表中某条目的同步进度。
///
/// Shared by all session_usage_* parsers.
pub(crate) fn get_sync_state(db: &Database, file_path: &str) -> Result<(i64, i64), AppError> {
    let conn = lock_conn!(db.conn);
    let result = conn.query_row(
        "SELECT last_modified, last_line_offset FROM session_log_sync WHERE file_path = ?1",
        rusqlite::params![file_path],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    Ok(result.unwrap_or((0, 0)))
}

/// 返回文件 mtime 的纳秒时间戳。
///
/// `session_log_sync.last_modified` 旧数据是秒级时间戳；新写入纳秒值不需要
/// schema 迁移，旧值会自然触发一次增量重扫，并继续依赖行 offset 避免重复导入。
pub(crate) fn metadata_modified_nanos(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// 更新 session_log_sync 表中某条目的同步进度。
///
/// Shared by all session_usage_* parsers.
pub(crate) fn update_sync_state(
    db: &Database,
    file_path: &str,
    last_modified: i64,
    last_offset: i64,
) -> Result<(), AppError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let conn = lock_conn!(db.conn);
    conn.execute(
        "INSERT OR REPLACE INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![file_path, last_modified, last_offset, now],
    )
    .map_err(|e| AppError::Database(format!("更新同步状态失败: {e}")))?;
    Ok(())
}

/// 插入单条会话日志到 proxy_request_logs，返回是否成功插入 (true=新插入, false=已存在)
fn insert_session_log_entry(
    db: &Database,
    request_id: &str,
    msg: &ParsedAssistantUsage,
) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);

    let created_at = msg
        .timestamp
        .as_ref()
        .and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.timestamp())
        })
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

    let dedup_key = DedupKey {
        app_type: "claude",
        model: &msg.model,
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        created_at,
    };
    if should_skip_session_insert(&conn, request_id, &dedup_key)? {
        return Ok(false);
    }

    // 计算费用
    let usage = TokenUsage {
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        model: Some(msg.model.clone()),
        message_id: None,
    };

    let pricing = find_model_pricing_for_session(&conn, &msg.model);
    let multiplier = Decimal::from(1);
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing
    {
        Some(p) => {
            let cost = CostCalculator::calculate(&usage, &p, multiplier);
            (
                cost.input_cost.to_string(),
                cost.output_cost.to_string(),
                cost.cache_read_cost.to_string(),
                cost.cache_creation_cost.to_string(),
                cost.total_cost.to_string(),
            )
        }
        None => (
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ),
    };

    let inserted_rows = conn
        .execute(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            rusqlite::params![
                request_id,
                "_session",         // provider_id: 标记为会话来源
                "claude",           // app_type
                msg.model,
                msg.model,          // request_model = model
                msg.input_tokens,
                msg.output_tokens,
                msg.cache_read_tokens,
                msg.cache_creation_tokens,
                input_cost,
                output_cost,
                cache_read_cost,
                cache_creation_cost,
                total_cost,
                0i64,               // latency_ms: 会话日志无此数据
                Option::<i64>::None, // first_token_ms
                200i64,             // status_code: 会话日志中的请求只要产生计费 token 即视为成功
                Option::<String>::None, // error_message
                msg.session_id,
                Some("session_log"), // provider_type
                1i64,               // is_streaming: Claude Code 通常使用流式
                "1.0",              // cost_multiplier
                created_at,
                "session_log",      // data_source
            ],
        )
        .map_err(|e| AppError::Database(format!("插入会话日志失败: {e}")))?;

    // 仅在确实写入新行时通知前端，避免 INSERT OR IGNORE 跳过时产生空刷新
    if inserted_rows > 0 {
        crate::usage_events::notify_log_recorded();
    }

    Ok(true)
}

/// 从 model_pricing 表查找模型定价（支持模糊匹配）
fn find_model_pricing_for_session(
    conn: &rusqlite::Connection,
    model_id: &str,
) -> Option<ModelPricing> {
    find_model_pricing(conn, model_id)
}

/// 查询数据来源分布统计
pub fn get_data_source_breakdown(db: &Database) -> Result<Vec<DataSourceSummary>, AppError> {
    let conn = lock_conn!(db.conn);

    let effective_filter = effective_usage_log_filter("l");
    let sql = format!(
        "SELECT COALESCE(l.data_source, 'proxy') as ds, COUNT(*) as cnt,
                COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as cost
         FROM proxy_request_logs l
         WHERE {effective_filter}
         GROUP BY ds
         ORDER BY cnt DESC"
    );

    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        Ok(DataSourceSummary {
            data_source: row.get(0)?,
            request_count: row.get::<_, i64>(1)? as u32,
            total_cost_usd: format!("{:.6}", row.get::<_, f64>(2)?),
        })
    })?;

    let mut summaries = Vec::new();
    for row in rows {
        summaries.push(row.map_err(|e| AppError::Database(e.to_string()))?);
    }

    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_usage_from_jsonl_line() {
        let line = r#"{"type":"assistant","message":{"id":"msg_test123","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":5000,"cache_creation_input_tokens":10000},"stop_reason":"end_turn"},"timestamp":"2026-04-05T12:00:00Z","sessionId":"session-abc"}"#;

        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            value.get("type").and_then(|t| t.as_str()),
            Some("assistant")
        );

        let message = value.get("message").unwrap();
        let usage = message.get("usage").unwrap();

        assert_eq!(usage.get("input_tokens").unwrap().as_u64().unwrap(), 3);
        assert_eq!(usage.get("output_tokens").unwrap().as_u64().unwrap(), 150);
        assert_eq!(
            usage
                .get("cache_read_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            5000
        );
        assert_eq!(
            usage
                .get("cache_creation_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            10000
        );
        assert_eq!(
            message.get("stop_reason").unwrap().as_str().unwrap(),
            "end_turn"
        );
    }

    #[test]
    fn test_dedup_by_message_id() {
        // 同一个 message.id 有多条，应该取 stop_reason 有值的那条
        let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();

        // 中间条目（无 stop_reason）
        let intermediate = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-6".to_string(),
            input_tokens: 3,
            output_tokens: 26,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            stop_reason: None,
            timestamp: Some("2026-04-05T12:00:00Z".to_string()),
            session_id: None,
        };
        messages.insert("msg_1".to_string(), intermediate);

        // 最终条目（有 stop_reason）
        let final_entry = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-6".to_string(),
            input_tokens: 3,
            output_tokens: 1349,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("2026-04-05T12:00:00Z".to_string()),
            session_id: None,
        };

        // 应该替换
        let should_replace = final_entry.stop_reason.is_some()
            && messages.get("msg_1").unwrap().stop_reason.is_none();
        assert!(should_replace);

        messages.insert("msg_1".to_string(), final_entry);
        assert_eq!(messages.get("msg_1").unwrap().output_tokens, 1349);
    }

    #[test]
    fn test_insert_claude_session_skips_matching_proxy_log() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "proxy-different-id",
                    "openai-compatible",
                    "claude",
                    "claude-sonnet-4-5",
                    "claude-sonnet-4-5",
                    100,
                    20,
                    10,
                    5,
                    "0.10",
                    100,
                    200,
                    1000,
                    "proxy"
                ],
            )?;
        }

        let msg = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("1970-01-01T00:16:45Z".to_string()),
            session_id: Some("session-1".to_string()),
        };

        let inserted = insert_session_log_entry(&db, "session:msg_1", &msg)?;
        assert!(!inserted);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

        Ok(())
    }

    #[test]
    fn test_collect_jsonl_files_includes_subagents() {
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        let project = tmp.join("project");
        let session_dir = project.join("test-session");
        let subagents_dir = session_dir.join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();

        fs::write(project.join("main.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("agent-abc.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(&tmp);
        assert_eq!(files.len(), 2);
        let paths: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.contains("main.jsonl")));
        assert!(paths.iter().any(|p| p.contains("agent-abc.jsonl")));

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_collect_jsonl_files_includes_workflow_subagents() {
        // Claude Code Workflow 把子 agent transcript 嵌在
        // 项目/SESSION_ID/subagents/workflows/wf_<ID>/ 下，比普通子 agent 深一层。
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        let project = tmp.join("project");
        let session_dir = project.join("test-session");
        let subagents_dir = session_dir.join("subagents");
        let wf_dir = subagents_dir.join("workflows").join("wf_test123");
        fs::create_dir_all(&wf_dir).unwrap();

        fs::write(project.join("main.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("agent-plain.jsonl"), "{}").unwrap();
        fs::write(wf_dir.join("agent-wf.jsonl"), "{}").unwrap();
        // journal.jsonl 也会被收集，但解析时因无 assistant 行而产出 0 条
        fs::write(wf_dir.join("journal.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(&tmp);
        let paths: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        // 主会话 + 普通子 agent + Workflow 子 agent(agent-wf + journal) = 4
        assert_eq!(files.len(), 4);
        assert!(paths.iter().any(|p| p.contains("main.jsonl")));
        assert!(paths.iter().any(|p| p.contains("agent-plain.jsonl")));
        assert!(
            paths.iter().any(|p| p.contains("agent-wf.jsonl")),
            "Workflow 子 agent transcript 必须被收集"
        );

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_sync_imports_billable_message_without_stop_reason() -> Result<(), AppError> {
        // 回归：stop_reason 缺失但有真实 cache/input 成本的 message（Workflow /
        // 子 agent 常见的「只有 message_start 快照、没写最终块」形态）必须被计入，
        // 不能因缺 stop_reason 或 output==0 而整条丢弃；全 0 token 的占位行仍应跳过。
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("agent-wf.jsonl");

        // 第一行：无 stop_reason、output=1，但 cache_read/cache_creation 很大 → 应导入
        // 第二行：全部 token 为 0 → 应跳过（无计费意义）
        let billable = r#"{"type":"assistant","message":{"id":"msg_nostop","model":"claude-opus-4-8","usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":48719,"cache_creation_input_tokens":2061}},"timestamp":"2026-06-07T13:01:23Z","sessionId":"session-wf"}"#;
        let empty = r#"{"type":"assistant","message":{"id":"msg_empty","model":"claude-opus-4-8","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"timestamp":"2026-06-07T13:01:24Z","sessionId":"session-wf"}"#;
        fs::write(&file, format!("{billable}\n{empty}\n")).unwrap();

        let (imported, _skipped) = sync_single_file(&db, &file)?;
        assert_eq!(
            imported, 1,
            "有 cache 成本但无 stop_reason 的 message 必须被导入"
        );

        let conn = lock_conn!(db.conn);
        let cache_read: i64 = conn.query_row(
            "SELECT cache_read_tokens FROM proxy_request_logs WHERE request_id = 'session:msg_nostop'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(cache_read, 48719, "cache_read 必须被完整记录");
        let empty_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM proxy_request_logs WHERE request_id = 'session:msg_empty')",
            [],
            |row| row.get(0),
        )?;
        assert!(!empty_exists, "全 0 token 的 message 应被跳过");
        drop(conn);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_encode_cwd_to_claude_dir() {
        // 对照用户真实环境验证编码（含 `_ → -` 这一非显然规则）
        assert_eq!(encode_cwd_to_claude_dir(r"D:\work\slg"), "D--work-slg");
        assert_eq!(
            encode_cwd_to_claude_dir(r"D:\work\slg_google_beta"),
            "D--work-slg-google-beta"
        );
        assert_eq!(
            encode_cwd_to_claude_dir(r"D:\work\slg-ai"),
            "D--work-slg-ai"
        );
        assert_eq!(encode_cwd_to_claude_dir(r"D:\work\slg2"), "D--work-slg2");
        assert_eq!(
            encode_cwd_to_claude_dir(r"E:\Projects\cc-switch"),
            "E--Projects-cc-switch"
        );
    }

    #[test]
    fn test_parse_assistant_messages_in_file_dedup_and_filters() {
        let tmp = std::env::temp_dir().join(format!("cc-switch-parse-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session-1.jsonl");

        // 四行：system 行（跳过）/ 同 id 两条 assistant（去重取 output 大的）/ 全 0 token（过滤）
        let system = r#"{"type":"system","message":{"id":"m_sys"}}"#;
        let lo = r#"{"type":"assistant","message":{"id":"msg_dup","model":"claude-sonnet-4-5","usage":{"input_tokens":3,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"timestamp":"2026-06-29T12:00:00Z"}"#;
        let hi = r#"{"type":"assistant","message":{"id":"msg_dup","model":"claude-sonnet-4-5","usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"stop_reason":"end_turn"},"timestamp":"2026-06-29T12:00:00Z"}"#;
        let zero = r#"{"type":"assistant","message":{"id":"msg_zero","model":"claude-sonnet-4-5","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"timestamp":"2026-06-29T12:01:00Z"}"#;
        fs::write(&file, format!("{system}\n{lo}\n{hi}\n{zero}\n")).unwrap();

        let msgs = parse_assistant_messages_in_file(&file);
        assert_eq!(msgs.len(), 1, "去重后只剩 1 条计费消息（全 0 行被过滤）");
        let m = &msgs[0];
        assert_eq!(m.message_id, "msg_dup");
        assert_eq!(m.output_tokens, 150, "同 id 取 output 较大者");
        assert_eq!(m.model, "claude-sonnet-4-5");
        assert_eq!(m.timestamp.as_deref(), Some("2026-06-29T12:00:00Z"));

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_update_project_id_by_fingerprint_matches() -> Result<(), AppError> {
        let db = Database::memory()?;
        let ts = "2026-06-29T12:00:00Z";
        let created_at = chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .timestamp();

        // msg 指纹：claude / sonnet / (in=100,out=50,cr=10,cc=5) / ts
        let msg = ParsedAssistantUsage {
            message_id: "msg_x".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some(ts.to_string()),
            session_id: None,
        };

        // 四行：完全命中 / token 不同 / 非 Claude / 时间窗外
        {
            let conn = lock_conn!(db.conn);
            for (rid, app_type, inp, c_at) in [
                ("req-hit", "claude", 100i64, created_at),
                ("req-wrong-tokens", "claude", 999, created_at),
                ("req-codex", "codex", 100, created_at),
                ("req-out-of-window", "claude", 100, created_at + 3600),
            ] {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model, request_model,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        total_cost_usd, latency_ms, status_code, created_at, data_source
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        rid,
                        "provider-1",
                        app_type,
                        "claude-sonnet-4-5",
                        "claude-sonnet-4-5",
                        inp,
                        50,
                        10,
                        5,
                        "0.01",
                        100,
                        200,
                        c_at,
                        "proxy",
                    ],
                )?;
            }
        }
        // 释放 DB 锁后再调用：update_project_id_by_fingerprint 内部自行加锁，重入会死锁
        let updated = update_project_id_by_fingerprint(&db, "proj-slg", &msg)?;
        assert_eq!(updated, 1, "只有指纹完全命中的行被回填");

        let conn = lock_conn!(db.conn);
        let pid_of = |rid: &str| -> Option<String> {
            conn.query_row(
                "SELECT project_id FROM proxy_request_logs WHERE request_id = ?1",
                [rid],
                |row| row.get(0),
            )
            .ok()
        };
        assert_eq!(pid_of("req-hit").as_deref(), Some("proj-slg"));
        assert!(pid_of("req-wrong-tokens").is_none(), "token 不匹配不应回填");
        assert!(pid_of("req-codex").is_none(), "非 Claude 行不应回填");
        assert!(pid_of("req-out-of-window").is_none(), "时间窗外不应回填");
        Ok(())
    }
}
