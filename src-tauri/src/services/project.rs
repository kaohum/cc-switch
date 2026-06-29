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
    build_effective_settings_with_common_config, sanitize_claude_settings_for_live,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
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
    /// 校验 provider 存在；绑定后 best-effort 写入项目根 .claude/settings.local.json。
    pub fn set_claude_provider(
        db: &Database,
        id: &str,
        provider_id: Option<&str>,
    ) -> Result<Project, AppError> {
        Self::require_project(db, id)?;

        let normalized = provider_id.map(|p| p.trim()).filter(|p| !p.is_empty());

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
        } else {
            db.update_project_provider(id, None, now_millis())?;
        }

        // best-effort 写入项目根 .claude/settings.local.json（路径不存在时只 warn，
        // 不阻塞绑定；用户可稍后用 write_project_claude_settings 手动重试）
        if normalized.is_some() {
            if let Err(e) = Self::write_claude_to_project(db, id) {
                log::warn!(
                    "项目 {id} 绑定 provider 后写入 .claude/settings.local.json 失败（可稍后手动重试）: {e}"
                );
            }
        }
        Self::require_project(db, id)
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
    /// **合并模式**而非覆盖：先读现有 settings.local.json（不存在则空对象），
    /// 只覆盖/追加 cc-switch 管理的 `env` 段（`ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`
    /// / `ANTHROPIC_DEFAULT_*_MODEL` 等），保留用户原有所有其他字段（hooks / enabledPlugins
    /// / permissions 等）。写前备份到 `settings.local.json.ccs.bak`，原子写入。
    ///
    /// 选 `settings.local.json` 而非 `settings.local.json` 的原因：Claude Code 官方约定
    /// local > project > user，本地个人配置默认 .gitignore，团队共享的 settings.local.json
    /// 不会被 cc-switch 污染。
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

        // 3) 合并：existing + sanitized.ccs 子段（env 整体覆盖，其他字段不碰）
        // cc-switch 只管理 `env` 段；其他字段（hooks / enabledPlugins / permissions / mcpServers 等）
        // 用户可自由编辑，cc-switch 不触碰
        let sanitized_env = sanitized
            .as_object()
            .and_then(|o| o.get("env"))
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        {
            let existing_obj = existing.as_object_mut().expect("existing is object");
            existing_obj.insert("env".into(), sanitized_env);
        }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::Provider;
    use serde_json::json;
    use tempfile::TempDir;

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
        assert_eq!(bound.claude_provider_id.as_deref(), Some("packy"));
    }

    #[test]
    fn set_claude_provider_clears_with_none() {
        let db = Database::memory().expect("memory db");
        seed_claude_provider(&db, "packy");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        ProjectService::set_claude_provider(&db, &p.id, Some("packy")).expect("bind");

        let cleared = ProjectService::set_claude_provider(&db, &p.id, None).expect("clear");
        assert!(cleared.claude_provider_id.is_none());
    }

    #[test]
    fn set_claude_provider_treats_empty_string_as_clear() {
        let db = Database::memory().expect("memory db");
        seed_claude_provider(&db, "packy");
        let p = ProjectService::create(&db, req("A", "/a")).expect("create");
        ProjectService::set_claude_provider(&db, &p.id, Some("packy")).expect("bind");

        let cleared = ProjectService::set_claude_provider(&db, &p.id, Some("   ")).expect("clear");
        assert!(cleared.claude_provider_id.is_none());
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
        assert_eq!(v["env"]["ANTHROPIC_BASE_URL"], "https://x.example");
        assert_eq!(v["env"]["ANTHROPIC_AUTH_TOKEN"], "tok");

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
}
