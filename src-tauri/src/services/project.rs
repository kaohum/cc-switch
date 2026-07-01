//! 项目工程目录管理 service
//!
//! 业务逻辑层：输入校验、ID/时间戳生成、调用 DAO。DAO 只管持久化，
//! service 负责校验和编排。方法接受 `&Database`（不依赖 Tauri state），
//! 便于用 `Database::memory()` 做单元测试。
//!
//! `set_claude_provider` 绑定后 best-effort 写入项目根 `.claude/settings.local.json`
//! （路径不存在时只 warn，可用 `write_project_claude_settings` 手动重试）。

use crate::app_config::AppType;
use crate::database::Database;
use crate::database::Project;
use crate::error::AppError;
use crate::services::provider::{
    build_effective_settings_with_common_config, json_deep_merge, sanitize_claude_settings_for_live,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 把完整的 cc-switch 生效配置（`sanitized`：provider 的 `env` + common config 全部字段）
/// 合并进项目已有的 settings.local.json（`existing`）。
///
/// - `env`：整体替换（provider 身份整体性，切换时不残留旧 `ANTHROPIC_*` key）。
/// - 其他 top-level 字段：深度合并——对象递归合并保留 existing 已有子项，标量/数组由 sanitized
///   覆盖（即 common config 优先）；复用 `json_deep_merge`，与全局 common config 合并语义一致。
/// - existing 独有、sanitized 未定义的字段：原样保留（用户项目级个性化配置）。
fn merge_full_settings_into_existing(existing: &mut Value, sanitized: &Value) {
    let Some(existing_obj) = existing.as_object_mut() else {
        return;
    };
    let Some(sanitized_obj) = sanitized.as_object() else {
        return;
    };
    for (key, sanitized_value) in sanitized_obj {
        if key == "env" {
            existing_obj.insert(key.clone(), sanitized_value.clone());
            continue;
        }
        match existing_obj.get_mut(key) {
            Some(existing_value) => json_deep_merge(existing_value, sanitized_value),
            None => {
                existing_obj.insert(key.clone(), sanitized_value.clone());
            }
        }
    }
}

/// 创建项目请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
}

/// 更新项目请求（所有字段可选，None 表示不改）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
}

/// 设置/解绑项目 Claude provider 的结果。
///
/// 绑定/解绑**始终**先落库（DB 是 SSOT）；项目根 `.claude/settings.local.json`
/// 的写入是 best-effort——成功时填 `written_path`，失败时填 `write_warning`
/// （前端据此 toast 提示，不再静默吞错）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderResult {
    pub project: Project,
    /// 实际写入的 settings.local.json 路径（写入成功时）。
    /// 解绑删除文件、或文件本就不存在时为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_path: Option<String>,
    /// best-effort 写入失败的可读原因（绑定/解绑本身已成功落库）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_warning: Option<String>,
}

pub struct ProjectService;

impl ProjectService {
    pub fn list(db: &Database, include_deleted: bool) -> Result<Vec<Project>, AppError> {
        db.list_projects(include_deleted)
    }

    pub fn get(db: &Database, id: &str) -> Result<Option<Project>, AppError> {
        db.get_project(id)
    }

    pub fn create(db: &Database, req: CreateProjectRequest) -> Result<Project, AppError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(AppError::localized(
                "project.name_required",
                "项目名称不能为空",
                "Project name is required",
            ));
        }
        let path = req.path.trim();
        if path.is_empty() {
            return Err(AppError::localized(
                "project.path_required",
                "项目路径不能为空",
                "Project path is required",
            ));
        }

        let now = now_millis();
        let project = Project {
            id: new_id(),
            name: name.to_string(),
            path: path.to_string(),
            description: req.description.filter(|s| !s.trim().is_empty()),
            claude_provider_id: req.claude_provider_id.filter(|s| !s.trim().is_empty()),
            created_at: now,
            updated_at: now,
            last_written_at: None,
            deleted_at: None,
            sort_index: Some(db.next_project_sort_index()?),
            icon: req.icon.filter(|s| !s.trim().is_empty()),
            icon_color: req.icon_color.filter(|s| !s.trim().is_empty()),
        };
        db.save_project(&project)?;

        // 新建时若已绑定 provider，best-effort 写入项目根（与 set_claude_provider
        // 一致；路径不存在时只 warn，用户可稍后用 write_project_claude_settings 重试）
        if project.claude_provider_id.is_some() {
            if let Err(e) = Self::write_claude_to_project(db, &project.id) {
                log::warn!(
                    "项目 {} 创建后写入 .claude/settings.local.json 失败（可稍后手动重试）: {e}",
                    project.id
                );
            }
        }
        Ok(project)
    }

    pub fn update(db: &Database, id: &str, req: UpdateProjectRequest) -> Result<Project, AppError> {
        let mut project = Self::require_project(db, id)?;
        if let Some(name) = req.name {
            let name = name.trim();
            if name.is_empty() {
                return Err(AppError::localized(
                    "project.name_required",
                    "项目名称不能为空",
                    "Project name is required",
                ));
            }
            project.name = name.to_string();
        }
        if let Some(path) = req.path {
            let path = path.trim();
            if path.is_empty() {
                return Err(AppError::localized(
                    "project.path_required",
                    "项目路径不能为空",
                    "Project path is required",
                ));
            }
            project.path = path.to_string();
        }
        if let Some(desc) = req.description {
            project.description = if desc.trim().is_empty() {
                None
            } else {
                Some(desc)
            };
        }
        if let Some(icon) = req.icon {
            project.icon = if icon.trim().is_empty() {
                None
            } else {
                Some(icon)
            };
        }
        if let Some(color) = req.icon_color {
            project.icon_color = if color.trim().is_empty() {
                None
            } else {
                Some(color)
            };
        }
        project.updated_at = now_millis();
        db.save_project(&project)?;
        Ok(project)
    }

    /// 软删除项目（设置 deleted_at，不物理删除）
    pub fn delete(db: &Database, id: &str) -> Result<(), AppError> {
        Self::require_project(db, id)?;
        db.soft_delete_project(id, now_millis())
    }

    pub fn restore(db: &Database, id: &str) -> Result<(), AppError> {
        db.restore_project(id)
    }

    /// 设置项目绑定的 Claude provider。
    ///
    /// - 绑定（`Some`）：校验 provider 存在 → 落库 → best-effort 写入项目根
    ///   `.claude/settings.local.json`（proxy 模式下含 `claude-*` 别名改写）。
    /// - 解绑（`None`）：落库 → best-effort **移除** cc-switch 管理的 `env` 段
    ///   （避免残留已失效的项目代理端点，导致 Claude Code 报 NoProvidersConfigured）。
    ///
    /// 落库始终先于写盘完成；写盘失败不回滚绑定，而是把原因填进
    /// [`SetProviderResult::write_warning`] 让前端提示。返回值始终 `Ok`（除非
    /// 项目/provider 校验失败）。
    pub fn set_claude_provider(
        db: &Database,
        id: &str,
        provider_id: Option<&str>,
    ) -> Result<SetProviderResult, AppError> {
        Self::require_project(db, id)?;

        let normalized = provider_id.map(|p| p.trim()).filter(|p| !p.is_empty());

        // 落库 + 对应的写盘操作（写盘是 best-effort，失败在下方转成 warning）
        let write_outcome: Result<Option<std::path::PathBuf>, AppError> =
            if let Some(pid) = normalized {
                // 引用完整性校验：provider 必须存在于 claude app_type
                if db
                    .get_provider_by_id(pid, AppType::Claude.as_str())?
                    .is_none()
                {
                    return Err(AppError::localized(
                        "project.provider_not_found",
                        format!("Claude 供应商 {pid} 不存在"),
                        format!("Claude provider {pid} not found"),
                    ));
                }
                db.update_project_provider(id, Some(pid), now_millis())?;
                Self::write_claude_to_project(db, id).map(Some)
            } else {
                db.update_project_provider(id, None, now_millis())?;
                Self::strip_claude_env_from_project(db, id)
            };

        let project = Self::require_project(db, id)?;
        let (written_path, write_warning) = match write_outcome {
            Ok(path) => (path.map(|p| p.to_string_lossy().to_string()), None),
            Err(e) => {
                let msg = e.to_string();
                log::warn!(
                    "项目 {id} settings.local.json 写入失败（绑定/解绑已落库，可稍后手动重试）: {msg}"
                );
                (None, Some(msg))
            }
        };
        Ok(SetProviderResult {
            project,
            written_path,
            write_warning,
        })
    }

    /// 取项目，不存在则报错（含已软删的，由调用方决定语义）
    fn require_project(db: &Database, id: &str) -> Result<Project, AppError> {
        db.get_project(id)?.ok_or_else(|| {
            AppError::localized(
                "project.not_found",
                format!("项目 {id} 不存在"),
                format!("Project {id} not found"),
            )
        })
    }

    /// 把项目绑定的 Claude provider 写入 `<项目根>/.claude/settings.local.json`。
    ///
    /// **合并模式**而非整体覆盖：先读现有 settings.local.json（不存在则空对象），再把完整的
    /// cc-switch 生效配置（provider 的 `env` + common config 的全部字段，proxy 模式下含
    /// BASE_URL/AUTH_TOKEN 覆盖）合并进去：
    /// - `env` 整体替换——切换 provider 时必须整套换，避免旧 provider 的 `ANTHROPIC_*` 残留；
    /// - 其他 top-level 字段（`effortLevel` / `enabledPlugins` / `mcpServers` / `statusLine` 等）
    ///   深度合并——保留项目已有的个性化项，同名冲突时 common config 优先；
    /// - existing 独有、cc-switch 未管理的字段（如用户自加的 `hooks` / `enabledMcpjsonServers`）原样保留。
    ///
    /// 写前备份到 `settings.local.json.ccs.bak`，原子写入。选 `settings.local.json` 而非
    /// `settings.json`：Claude Code 官方约定 local > project > user，本地个人配置默认 .gitignore，
    /// 团队共享的 `settings.json` 不会被 cc-switch 污染。
    pub fn write_claude_to_project(
        db: &Database,
        project_id: &str,
    ) -> Result<std::path::PathBuf, AppError> {
        let project = Self::require_project(db, project_id)?;
        let provider_id = project.claude_provider_id.as_deref().ok_or_else(|| {
            AppError::localized(
                "project.no_provider",
                "项目未绑定 Claude provider",
                "Project has no Claude provider bound",
            )
        })?;
        let provider = db
            .get_provider_by_id(provider_id, AppType::Claude.as_str())?
            .ok_or_else(|| {
                AppError::localized(
                    "project.provider_not_found",
                    format!("Claude 供应商 {provider_id} 不存在"),
                    format!("Claude provider {provider_id} not found"),
                )
            })?;

        let project_root = std::path::PathBuf::from(&project.path);
        if !project_root.is_dir() {
            return Err(AppError::Config(format!(
                "项目路径不存在或不是目录: {}",
                project_root.display()
            )));
        }
        let claude_dir = project_root.join(".claude");
        std::fs::create_dir_all(&claude_dir).map_err(|e| AppError::io(&claude_dir, e))?;

        let settings_path = claude_dir.join("settings.local.json");

        // 1) 读现有 settings.local.json（保留用户其他配置）；不存在则空对象
        let mut existing: Value = if settings_path.exists() {
            let raw = std::fs::read_to_string(&settings_path)
                .map_err(|e| AppError::io(&settings_path, e))?;
            serde_json::from_str(&raw).unwrap_or(Value::Object(Map::new()))
        } else {
            Value::Object(Map::new())
        };
        if !existing.is_object() {
            // 存在但不是对象（如用户写成数组）→ 备份后当作空对象，避免覆盖用户数据
            log::warn!(
                "{} 顶层不是 JSON 对象，按空对象合并（已备份为 .bak）",
                settings_path.display()
            );
            let backup = settings_path.with_extension("json.ccs.bak");
            let _ = std::fs::copy(&settings_path, &backup);
            existing = Value::Object(Map::new());
        }

        // 写前备份原文件
        if settings_path.exists() {
            let backup = claude_dir.join("settings.local.json.ccs.bak");
            if let Err(e) = std::fs::copy(&settings_path, &backup) {
                log::warn!("备份 {} 失败: {e}", settings_path.display());
            }
        }

        // 2) 构造 cc-switch 管理的 effective settings + sanitize
        let effective =
            build_effective_settings_with_common_config(db, &AppType::Claude, &provider)?;
        let mut sanitized = sanitize_claude_settings_for_live(&effective);

        // proxy 模式（方案 A）：覆盖 base_url/token 指向 cc-switch proxy + 项目路径
        if crate::settings::get_settings().enable_local_proxy {
            // 保留 provider 的具体模型名原样写入——Claude Code /model 菜单据此显示
            // 真实模型名（如 glm-4.5-air）；路由 correctness 由 model_mapper 的
            // is_configured_target 幂等保护承担（发出的具体名命中已配置目标即透传）。
            // 不写 claude-* 别名：那是 cc-switch 自定义短别名，CC 不识别会显示原始串。
            let listen = futures::executor::block_on(db.get_global_proxy_config()).ok();
            let host = listen
                .as_ref()
                .map(|c| c.listen_address.as_str())
                .unwrap_or("127.0.0.1");
            let port = listen.as_ref().map(|c| c.listen_port).unwrap_or(15721);
            let connect_host = match host {
                "0.0.0.0" => "127.0.0.1",
                other => other,
            };
            let project_url = format!("http://{connect_host}:{port}/claude/project/{}", project.id);
            let token = format!("ccs-project-{}", &project.id);
            // 收集 effective 的 env 段（去掉 .ccs 项目特定覆盖，让 sanitized env 保持
            // provider 原始字段；proxy URL/token 在合并阶段最后单独写）
            let env_obj = sanitized
                .as_object_mut()
                .and_then(|o| o.get_mut("env"))
                .and_then(|v| v.as_object_mut());
            if let Some(env) = env_obj {
                env.insert(
                    "ANTHROPIC_BASE_URL".into(),
                    Value::String(project_url.clone()),
                );
                env.insert("ANTHROPIC_AUTH_TOKEN".into(), Value::String(token));
            } else if let Some(obj) = sanitized.as_object_mut() {
                let mut env = Map::new();
                env.insert(
                    "ANTHROPIC_BASE_URL".into(),
                    Value::String(project_url.clone()),
                );
                env.insert("ANTHROPIC_AUTH_TOKEN".into(), Value::String(token));
                obj.insert("env".into(), Value::Object(env));
            }
            log::info!(
                "项目 '{}' proxy 模式 settings.local.json → {}",
                project.name,
                project_url
            );
        }

        // 3) 合并：完整的 sanitized（provider env + common config 全部字段，含 proxy 覆盖）
        // 并入 existing——env 整体替换，其他字段深度合并（common config 优先），existing 独有字段保留。
        // 详见 [`merge_full_settings_into_existing`]。
        merge_full_settings_into_existing(&mut existing, &sanitized);

        // 4) 原子写
        crate::config::write_json_file(&settings_path, &existing)?;

        db.update_project_last_written_at(project_id, now_millis())?;
        log::info!(
            "项目 '{}' 的 Claude settings 已合并写入 {}",
            project.name,
            settings_path.display()
        );
        Ok(settings_path)
    }

    /// 解绑时移除项目 `settings.local.json` 里 cc-switch 管理的 `env` 段。
    ///
    /// cc-switch 写盘时 `env` 是**整体替换**（见 [`merge_full_settings_into_existing`]），
    /// 故文件中的 `env` 全部由 cc-switch 管理，整体移除是安全的——不会动用户自加的
    /// 非 env 字段（`hooks` / `enabledMcpjsonServers` 等）。移除后若对象变空，说明
    /// 该文件是 cc-switch 新建的、无用户内容，直接删除以免残留空配置。文件不存在则无操作。
    ///
    /// 解绑后 Claude Code 不再指向已失效的项目代理端点（`/claude/project/<id>/`），
    /// 回退到全局配置，保持可用（"最小侵入"原则）。
    ///
    /// 返回实际写入路径（`Some`）；文件不存在或被删除时返回 `None`。
    fn strip_claude_env_from_project(
        db: &Database,
        project_id: &str,
    ) -> Result<Option<std::path::PathBuf>, AppError> {
        let project = Self::require_project(db, project_id)?;
        let settings_path = std::path::PathBuf::from(&project.path)
            .join(".claude")
            .join("settings.local.json");
        if !settings_path.exists() {
            return Ok(None);
        }

        let raw =
            std::fs::read_to_string(&settings_path).map_err(|e| AppError::io(&settings_path, e))?;
        let mut value: Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| Value::Object(Map::new()));

        let emptied = match value.as_object_mut() {
            Some(obj) => {
                obj.remove("env"); // cc-switch 管理的 env 整体移除
                obj.is_empty()
            }
            None => false, // 顶层非对象（用户自定义结构）：不敢动，原样保留
        };

        if emptied {
            // 只剩空对象 → cc-switch 新建的文件，删除以免残留空配置
            std::fs::remove_file(&settings_path).map_err(|e| AppError::io(&settings_path, e))?;
            log::info!(
                "项目 '{}' 解绑：settings.local.json 仅含 cc-switch env，已删除",
                project.name
            );
            Ok(None)
        } else {
            crate::config::write_json_file(&settings_path, &value)?;
            log::info!(
                "项目 '{}' 解绑：已移除 cc-switch 管理的 env 段，保留用户其它字段 → {}",
                project.name,
                settings_path.display()
            );
            Ok(Some(settings_path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::Provider;
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    /// 隔离 HOME → 让 `crate::settings::get_settings()` 读临时目录而非真实
    /// `~/.cc-switch/settings.json`。`write_claude_to_project` 会按 `enable_local_proxy`
    /// 走不同分支（直连 vs proxy 别名改写），不隔离会让测试依赖运行机器的真实设置。
    /// 与 `proxy::provider_router::tests::TempHome` 同构；用 `#[serial]` 串行避免污染。
    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        orig_home: Option<String>,
        orig_userprofile: Option<String>,
        orig_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("tmp");
            let orig_home = env::var("HOME").ok();
            let orig_userprofile = env::var("USERPROFILE").ok();
            let orig_test_home = env::var("CC_SWITCH_TEST_HOME").ok();
            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            crate::settings::reload_settings().expect("reload settings");
            Self {
                dir,
                orig_home,
                orig_userprofile,
                orig_test_home,
            }
        }

        /// 把指定 JSON 写进临时 home 的 settings.json 并 reload，用于强制开关
        /// （如 `enable_local_proxy`）。临时目录随 TempHome 释放而清理，不污染真实环境。
        fn write_settings(&self, settings_json: &Value) {
            let dir = self.dir.path().join(".cc-switch");
            std::fs::create_dir_all(&dir).expect("create .cc-switch");
            std::fs::write(
                dir.join("settings.json"),
                serde_json::to_string(settings_json).expect("serialize settings"),
            )
            .expect("write settings.json");
            crate::settings::reload_settings().expect("reload settings");
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.orig_home {
                Some(v) => env::set_var("HOME", v),
                None => env::remove_var("HOME"),
            }
            match &self.orig_userprofile {
                Some(v) => env::set_var("USERPROFILE", v),
                None => env::remove_var("USERPROFILE"),
            }
            match &self.orig_test_home {
                Some(v) => env::set_var("CC_SWITCH_TEST_HOME", v),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
            // 恢复进程级 settings 缓存到真实环境（忽略失败：仅测试整洁性）
            let _ = crate::settings::reload_settings();
        }
    }

    fn req(name: &str, path: &str) -> CreateProjectRequest {
        CreateProjectRequest {
            name: name.into(),
            path: path.into(),
            description: None,
            claude_provider_id: None,
            icon: None,
            icon_color: None,
        }
    }

    fn seed_claude_provider(db: &Database, id: &str) {
        let p = Provider::with_id(id.into(), id.into(), json!({ "env": {} }), None);
        db.save_provider(AppType::Claude.as_str(), &p)
            .expect("save provider");
    }

    #[test]
    fn create_project_success_assigns_id_and_sort_index() {
        let db = Database::memory().expect("memory db");
        let p = ProjectService::create(&db, req("SLG", "/work/slg")).expect("create");
        assert!(!p.id.is_empty());
        assert_eq!(p.name, "SLG");
        assert_eq!(p.path, "/work/slg");
        assert_eq!(p.sort_index, Some(0));
        assert!(p.deleted_at.is_none());
    }

    #[test]
    fn create_project_rejects_empty_name() {
        let db = Database::memory().expect("memory db");
        let err = ProjectService::create(&db, req("  ", "/x")).unwrap_err();
        assert!(matches!(err, AppError::Localized { .. }));
    }

    #[test]
    fn create_project_rejects_empty_path() {
        let db = Database::memory().expect("memory db");
        let err = ProjectService::create(&db, req("X", "")).unwrap_err();
        assert!(matches!(err, AppError::Localized { .. }));
    }

    #[test]
    fn create_project_increments_sort_index() {
        let db = Database::memory().expect("memory db");
        let a = ProjectService::create(&db, req("A", "/a")).expect("create a");
        let b = ProjectService::create(&db, req("B", "/b")).expect("create b");
        assert_eq!(a.sort_index, Some(0));
        assert_eq!(b.sort_index, Some(1));
    }

    #[test]
    fn update_project_changes_fields() {
        let db = Database::memory().expect("memory db");
        let p = ProjectService::create(&db, req("Old", "/old")).expect("create");
        let updated = ProjectService::update(
            &db,
            &p.id,
            UpdateProjectRequest {
                name: Some("New".into()),
                path: None,
                description: Some("desc".into()),
                icon: None,
                icon_color: None,
            },
        )
        .expect("update");
        assert_eq!(updated.name, "New");
        assert_eq!(updated.path, "/old"); // 未传 path 保持不变
        assert_eq!(updated.description.as_deref(), Some("desc"));
    }

    #[test]
    fn update_project_not_found() {
        let db = Database::memory().expect("memory db");
        let err = ProjectService::update(
            &db,
            "nope",
            UpdateProjectRequest {
                name: Some("X".into()),
                path: None,
                description: None,
                icon: None,
                icon_color: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, AppError::Localized { .. }));
    }

    #[test]
    fn delete_soft_deletes_then_list_excludes() {
        let db = Database::memory().expect("memory db");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        ProjectService::delete(&db, &p.id).expect("delete");

        assert!(ProjectService::list(&db, false).expect("list").is_empty());
        // include_deleted 仍能看到
        assert_eq!(ProjectService::list(&db, true).expect("list").len(), 1);
    }

    #[test]
    fn restore_project_brings_back_to_list() {
        let db = Database::memory().expect("memory db");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        ProjectService::delete(&db, &p.id).expect("delete");
        assert!(ProjectService::list(&db, false).expect("list").is_empty());

        ProjectService::restore(&db, &p.id).expect("restore");
        assert_eq!(ProjectService::list(&db, false).expect("list").len(), 1);
    }

    #[test]
    fn set_claude_provider_rejects_unknown_provider() {
        let db = Database::memory().expect("memory db");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        let err = ProjectService::set_claude_provider(&db, &p.id, Some("ghost")).unwrap_err();
        assert!(matches!(err, AppError::Localized { .. }));
    }

    #[test]
    fn set_claude_provider_binds_existing_provider() {
        let db = Database::memory().expect("memory db");
        seed_claude_provider(&db, "packy");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");

        let bound = ProjectService::set_claude_provider(&db, &p.id, Some("packy")).expect("bind");
        assert_eq!(bound.project.claude_provider_id.as_deref(), Some("packy"));
    }

    #[test]
    fn set_claude_provider_clears_with_none() {
        let db = Database::memory().expect("memory db");
        seed_claude_provider(&db, "packy");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        ProjectService::set_claude_provider(&db, &p.id, Some("packy")).expect("bind");

        let cleared = ProjectService::set_claude_provider(&db, &p.id, None).expect("clear");
        assert!(cleared.project.claude_provider_id.is_none());
    }

    #[test]
    fn set_claude_provider_treats_empty_string_as_clear() {
        let db = Database::memory().expect("memory db");
        seed_claude_provider(&db, "packy");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        ProjectService::set_claude_provider(&db, &p.id, Some("packy")).expect("bind");

        let cleared = ProjectService::set_claude_provider(&db, &p.id, Some("   ")).expect("clear");
        assert!(cleared.project.claude_provider_id.is_none());
    }

    /// 解绑时必须移除 cc-switch 管理的 env 段（避免残留失效的项目代理端点），
    /// 保留用户自加的非 env 字段；只剩空对象时删除文件。写盘失败要冒泡成 warning。
    #[test]
    #[serial]
    fn set_claude_provider_unbind_strips_env_but_keeps_user_fields() {
        let _home = TempHome::new();
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();
        seed_provider_with_env(&db, "packy", "https://x.example");

        let project = ProjectService::create(&db, req("A", &project_path)).expect("create");
        ProjectService::set_claude_provider(&db, &project.id, Some("packy")).expect("bind");

        let settings = dir.path().join(".claude").join("settings.local.json");
        assert!(settings.exists(), "绑定后应写入 settings.local.json");

        // 给文件加一个用户自有的非 env 字段，解绑时应保留
        {
            let raw = std::fs::read_to_string(&settings).unwrap();
            let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            v["enabledMcpjsonServers"] = json!(["my-tools"]);
            std::fs::write(&settings, serde_json::to_string(&v).unwrap()).unwrap();
        }

        let cleared =
            ProjectService::set_claude_provider(&db, &project.id, None).expect("clear unbind");
        assert!(cleared.project.claude_provider_id.is_none());
        assert!(
            cleared.write_warning.is_none(),
            "真实路径下解绑写盘应成功，无 warning"
        );

        // env 段被移除，但用户字段保留
        let raw = std::fs::read_to_string(&settings).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(v.get("env").is_none(), "解绑后 env 段必须移除");
        assert_eq!(
            v["enabledMcpjsonServers"],
            json!(["my-tools"]),
            "用户自加的非 env 字段必须保留"
        );
    }

    /// 解绑时若文件只含 cc-switch env（无用户字段），应直接删除文件。
    #[test]
    #[serial]
    fn set_claude_provider_unbind_deletes_file_when_only_managed_env() {
        let _home = TempHome::new();
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();
        seed_provider_with_env(&db, "packy", "https://y.example");

        let project = ProjectService::create(&db, req("A", &project_path)).expect("create");
        ProjectService::set_claude_provider(&db, &project.id, Some("packy")).expect("bind");

        let settings = dir.path().join(".claude").join("settings.local.json");
        assert!(settings.exists());

        ProjectService::set_claude_provider(&db, &project.id, None).expect("clear unbind");
        assert!(
            !settings.exists(),
            "文件只含 cc-switch env 时，解绑应删除文件，不留空配置"
        );
    }

    /// 写盘失败（项目路径不存在）时：绑定仍落库成功，且把失败原因冒泡到 write_warning，
    /// 返回 Ok 而非 Err（不阻塞绑定，前端据此 toast 提示）。
    #[test]
    #[serial]
    fn set_claude_provider_surfaces_write_failure_as_warning() {
        let _home = TempHome::new();
        let db = Database::memory().expect("memory db");
        seed_provider_with_env(&db, "packy", "https://z.example");
        // 路径不存在 → write_claude_to_project 返回 Config 错误
        let project =
            ProjectService::create(&db, req("A", "/nonexistent/ccs-test/xyz")).expect("create");

        let res = ProjectService::set_claude_provider(&db, &project.id, Some("packy"))
            .expect("bind still succeeds");
        assert_eq!(
            res.project.claude_provider_id.as_deref(),
            Some("packy"),
            "DB 绑定必须成功"
        );
        assert!(
            res.write_warning.is_some(),
            "路径不存在时写盘失败必须冒泡成 warning，不能静默"
        );
        assert!(
            res.written_path.is_none(),
            "写盘失败时 written_path 应为 None"
        );
    }

    fn seed_provider_with_env(db: &Database, id: &str, base_url: &str) {
        let provider = Provider::with_id(
            id.into(),
            id.into(),
            json!({ "env": { "ANTHROPIC_BASE_URL": base_url, "ANTHROPIC_AUTH_TOKEN": "tok" } }),
            None,
        );
        db.save_provider(AppType::Claude.as_str(), &provider)
            .expect("save provider");
    }

    #[test]
    fn write_claude_to_project_creates_settings_json_with_provider_env() {
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();
        seed_provider_with_env(&db, "packy", "https://x.example");

        let project = ProjectService::create(&db, req("A", &project_path)).expect("create");
        // set_claude_provider 绑定后 best-effort 写入（路径存在 → 成功）
        ProjectService::set_claude_provider(&db, &project.id, Some("packy")).expect("bind");

        let settings = dir.path().join(".claude").join("settings.local.json");
        assert!(
            settings.exists(),
            "绑定 provider 后应写入 settings.local.json"
        );
        let content = std::fs::read_to_string(&settings).expect("read");
        let v: serde_json::Value = serde_json::from_str(&content).expect("parse json");
        // proxy 开启时 BASE_URL/AUTH_TOKEN 会被改写为本地代理地址（ccs-project-<id>），
        // 关闭时为 provider 原值；两种情况下 env 段都应被写入，故只断言 key 存在且为非空字符串。
        assert!(
            v["env"]["ANTHROPIC_BASE_URL"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "env.ANTHROPIC_BASE_URL 应被写入"
        );
        assert!(
            v["env"]["ANTHROPIC_AUTH_TOKEN"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "env.ANTHROPIC_AUTH_TOKEN 应被写入"
        );

        let reloaded = ProjectService::get(&db, &project.id)
            .expect("get")
            .expect("present");
        assert!(reloaded.last_written_at.is_some(), "last_written_at 应更新");
    }

    #[test]
    fn write_claude_to_project_backs_up_existing_settings() {
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mkdir");
        std::fs::write(claude_dir.join("settings.local.json"), b"{\"old\": true}")
            .expect("write old");
        seed_provider_with_env(&db, "packy", "https://y");

        let project_path = dir.path().to_string_lossy().to_string();
        let project = ProjectService::create(&db, req("A", &project_path)).expect("create");
        ProjectService::set_claude_provider(&db, &project.id, Some("packy")).expect("bind");

        let backup = claude_dir.join("settings.local.json.ccs.bak");
        assert!(backup.exists(), "应备份旧 settings.local.json 到 .ccs.bak");
        let backup_content = std::fs::read_to_string(&backup).expect("read backup");
        assert!(backup_content.contains("old"), "备份应保留旧内容");

        // 合并模式：settings.local.json 应保留原有 "old" 字段，同时注入 env
        let merged =
            std::fs::read_to_string(claude_dir.join("settings.local.json")).expect("read merged");
        let v: serde_json::Value = serde_json::from_str(&merged).expect("parse merged");
        assert_eq!(
            v["old"], true,
            "合并模式应保留原有非 env 字段（hooks/plugins/permissions 等）"
        );
        assert!(
            v["env"]["ANTHROPIC_BASE_URL"].is_string(),
            "env 段应被注入（provider 配置）"
        );
    }

    #[test]
    fn write_claude_to_project_errors_when_path_missing() {
        let db = Database::memory().expect("memory db");
        seed_provider_with_env(&db, "packy", "https://z");
        let project =
            ProjectService::create(&db, req("A", "/nonexistent/ccs-test/xyz")).expect("create");
        // 路径不存在：set_claude_provider 的 best-effort 写入失败但绑定成功
        ProjectService::set_claude_provider(&db, &project.id, Some("packy")).expect("bind");

        let err = ProjectService::write_claude_to_project(&db, &project.id).unwrap_err();
        assert!(
            matches!(err, AppError::Config(_)),
            "路径不存在应返回 Config 错误"
        );
    }

    #[test]
    fn write_claude_to_project_errors_without_provider() {
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();
        let project = ProjectService::create(&db, req("A", &project_path)).expect("create");

        let err = ProjectService::write_claude_to_project(&db, &project.id).unwrap_err();
        assert!(
            matches!(err, AppError::Localized { .. }),
            "未绑定 provider 应返回 Localized 错误"
        );
    }

    #[test]
    fn create_with_provider_writes_settings_json() {
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();
        seed_provider_with_env(&db, "packy", "https://create-test");

        let project = ProjectService::create(
            &db,
            CreateProjectRequest {
                name: "A".into(),
                path: project_path,
                description: None,
                claude_provider_id: Some("packy".into()),
                icon: None,
                icon_color: None,
            },
        )
        .expect("create");

        let settings = dir.path().join(".claude").join("settings.local.json");
        assert!(
            settings.exists(),
            "create 带 provider 应自动写入项目根 settings.local.json"
        );
        let reloaded = ProjectService::get(&db, &project.id)
            .expect("get")
            .expect("present");
        assert!(
            reloaded.last_written_at.is_some(),
            "last_written_at 应被设置"
        );
    }

    /// common config 的非 env 字段（effortLevel / enabledPlugins / mcpServers 等）也应写入项目
    /// settings.local.json，且：
    /// - `env` 整体替换（切换 provider 不残留旧 key）；
    /// - 其他字段深度合并（保留项目已有个性化项，common config 优先）；
    /// - existing 独有字段保留。
    ///
    /// 直连模式（proxy 关）下 provider env 原样写入；proxy 模式的模型别名改写
    /// 另见 `write_claude_to_project_proxy_mode_rewrites_models_to_claude_aliases`。
    #[test]
    #[serial]
    fn write_claude_to_project_merges_full_provider_and_common_config() {
        // 隔离 settings：强制直连模式（proxy off），让 provider env 原样写入
        let _home = TempHome::new();
        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();

        // provider：只有 env
        let mut provider = Provider::with_id(
            "glm".into(),
            "glm".into(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic",
                    "ANTHROPIC_AUTH_TOKEN": "tok-glm",
                    "ANTHROPIC_MODEL": "glm-5.2"
                }
            }),
            None,
        );
        // 显式开启 common config（否则走 legacy subset 检测，env-only provider 不会命中）
        provider.meta = Some(crate::provider::ProviderMeta {
            common_config_enabled: Some(true),
            ..Default::default()
        });
        db.save_provider(AppType::Claude.as_str(), &provider)
            .expect("save provider");

        // common config snippet：env 之外的共享字段
        db.set_config_snippet(
            AppType::Claude.as_str(),
            Some(
                r#"{
                    "effortLevel": "max",
                    "enabledPlugins": { "superpowers@claude-plugins-official": true },
                    "mcpServers": {
                        "shared-server": { "type": "http", "url": "https://shared.example/mcp" }
                    }
                }"#
                .to_string(),
            ),
        )
        .expect("set common config snippet");

        // 项目已有的 settings.local.json：含用户个性化字段 + 一个将被整体替换的旧 env
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mkdir");
        std::fs::write(
            claude_dir.join("settings.local.json"),
            r#"{
                "enabledMcpjsonServers": ["fairygui-tools"],
                "hooks": { "user-hook": { "type": "command" } },
                "env": {
                    "STALE_FROM_OLD_PROVIDER": "should-be-removed",
                    "ANTHROPIC_MODEL": "old-model"
                },
                "mcpServers": {
                    "project-server": { "command": "npx", "args": ["x"] }
                }
            }"#,
        )
        .expect("write existing");

        let project = ProjectService::create(&db, req("SLG", &project_path)).expect("create");
        ProjectService::set_claude_provider(&db, &project.id, Some("glm")).expect("bind");

        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(claude_dir.join("settings.local.json")).expect("read"),
        )
        .expect("parse");

        // env 整体替换：旧 provider 残留 key 被清除（直连模式下 provider 的 MODEL 原样保留）
        assert!(
            v["env"].get("STALE_FROM_OLD_PROVIDER").is_none(),
            "env 应整体替换，旧 provider 的残留 key 必须清除"
        );
        assert_eq!(
            v["env"]["ANTHROPIC_MODEL"], "glm-5.2",
            "直连模式下 provider 的 env 应整体写入"
        );

        // common config 的非 env 字段应被写入
        assert_eq!(
            v["effortLevel"], "max",
            "common config 的 effortLevel 应写入"
        );
        assert_eq!(
            v["enabledPlugins"]["superpowers@claude-plugins-official"], true,
            "common config 的 enabledPlugins 应写入"
        );

        // existing 独有字段保留
        assert_eq!(
            v["enabledMcpjsonServers"],
            serde_json::json!(["fairygui-tools"]),
            "用户已有的 enabledMcpjsonServers 应保留"
        );
        assert_eq!(
            v["hooks"]["user-hook"]["type"], "command",
            "用户已有的 hooks 应保留"
        );

        // mcpServers 深度合并：项目的 project-server 保留 + common config 的 shared-server 补入
        assert!(
            v["mcpServers"].get("project-server").is_some(),
            "深度合并应保留项目已有的 mcpServers"
        );
        assert!(
            v["mcpServers"].get("shared-server").is_some(),
            "深度合并应补入 common config 的 mcpServers"
        );
    }

    /// proxy 模式下，项目 settings.local.json 必须保留 provider 的具体模型名原样写入
    /// （不改写成 claude-* 别名），这样 Claude Code /model 菜单显示真实模型名（如
    /// glm-4.5-air）。路由 correctness 由 model_mapper 的 is_configured_target 幂等
    /// 保护承担。仅 BASE_URL/TOKEN 覆盖为项目级代理端点。
    #[test]
    #[serial]
    fn write_claude_to_project_proxy_mode_keeps_concrete_model_names() {
        let home = TempHome::new();
        // 强制启用本地代理（项目级路由依赖 proxy 模式）。
        // AppSettings 用 serde rename_all="camelCase"，故键名是 enableLocalProxy。
        home.write_settings(&json!({ "enableLocalProxy": true }));

        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();

        // 复刻用户真实 GLM provider 配置（haiku=glm-4.5-air，default=glm-5.2）
        let provider = Provider::with_id(
            "glm".into(),
            "glm".into(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic",
                    "ANTHROPIC_AUTH_TOKEN": "tok-glm",
                    "ANTHROPIC_MODEL": "glm-5.2",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-4.5-air",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.1",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.2[1M]"
                }
            }),
            None,
        );
        db.save_provider(AppType::Claude.as_str(), &provider)
            .expect("save provider");

        let project = ProjectService::create(&db, req("SLG", &project_path)).expect("create");
        ProjectService::set_claude_provider(&db, &project.id, Some("glm")).expect("bind");

        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".claude").join("settings.local.json"))
                .expect("read"),
        )
        .expect("parse");
        let env = &v["env"];

        // 具体模型名原样保留（CC /model 菜单据此显示真实名）
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "glm-4.5-air");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "glm-5.1");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "glm-5.2[1M]");
        // 不写 claude-* 别名（cc-switch 自定义短别名 CC 不识别）
        assert!(
            !env["ANTHROPIC_DEFAULT_HAIKU_MODEL"]
                .as_str()
                .unwrap_or("")
                .starts_with("claude-"),
            "不应再写入 claude-* 别名，实际: {:?}",
            env["ANTHROPIC_DEFAULT_HAIKU_MODEL"]
        );
        // 不写 *_NAME 字段（具体名自显示，无需 _NAME）
        assert!(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME").is_none(),
            "不应写入 _NAME 字段"
        );
        // ANTHROPIC_MODEL（default 档具体名）保留，不再被移除
        assert_eq!(env["ANTHROPIC_MODEL"], "glm-5.2");
        // BASE_URL 指向项目级代理端点
        assert!(
            env["ANTHROPIC_BASE_URL"]
                .as_str()
                .is_some_and(|u| u.contains(&format!("/claude/project/{}", project.id))),
            "BASE_URL 应指向项目级代理端点，实际: {:?}",
            env["ANTHROPIC_BASE_URL"]
        );
    }
}
