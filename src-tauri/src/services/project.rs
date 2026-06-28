//! 项目工程目录管理 service
//!
//! 业务逻辑层：输入校验、ID/时间戳生成、调用 DAO。DAO 只管持久化，
//! service 负责校验和编排。方法接受 `&Database`（不依赖 Tauri state），
//! 便于用 `Database::memory()` 做单元测试。
//!
//! 写入项目根 `.claude/settings.json` 的逻辑在 M2 阶段加入；
//! 当前 `set_claude_provider` 只更新数据库绑定。

use crate::app_config::AppType;
use crate::database::Database;
use crate::database::Project;
use crate::error::AppError;
use serde::{Deserialize, Serialize};

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
    /// 校验 provider 存在；TODO(M2): 设置后自动写入项目根 .claude/settings.json。
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::provider::Provider;
    use serde_json::json;

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
}
