//! 项目工程目录 DAO
//!
//! 项目工程目录管理的数据访问层。每个项目绑定一个 Claude provider，
//! 用于实现「在不同项目目录启动的 Claude CLI 使用不同 provider」。
//!
//! 所有 DAO 方法通过 `impl Database` 提供。DAO 只管持久化，不做引用完整性
//! 校验（由 service 层负责），也不自己生成时间戳（由调用方传入，便于测试）。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// 项目工程目录（绑定一个 Claude provider）
///
/// 一个项目对应一个工作目录，绑定全局 Claude provider 池中的一个 provider。
/// 切换到项目时，service 层把绑定的 provider 写到 `<项目根>/.claude/settings.json`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 绑定的 Claude provider id（引用 providers.id, app_type='claude'）。
    /// 本 DAO 不验证引用完整性，由 service 层负责。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_provider_id: Option<String>,
    /// 创建时间（Unix 毫秒）
    pub created_at: i64,
    /// 最后更新时间（Unix 毫秒）
    pub updated_at: i64,
    /// 上次成功写入项目根 .claude/settings.json 的时间（Unix 毫秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_written_at: Option<i64>,
    /// 软删除时间（None = 未删除）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_index: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
}

/// 把 rusqlite 行映射成 Project（query_map 闭包要求返回 rusqlite::Result）
fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        description: row.get(3)?,
        claude_provider_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        last_written_at: row.get(7)?,
        deleted_at: row.get(8)?,
        sort_index: row.get(9)?,
        icon: row.get(10)?,
        icon_color: row.get(11)?,
    })
}

impl Database {
    /// UPSERT 项目（按 id 判断新增/更新）
    pub fn save_project(&self, project: &Project) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;

        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                params![project.id],
                |_| Ok(()),
            )
            .is_ok();

        if exists {
            tx.execute(
                "UPDATE projects SET
                    name = ?1,
                    path = ?2,
                    description = ?3,
                    claude_provider_id = ?4,
                    created_at = ?5,
                    updated_at = ?6,
                    last_written_at = ?7,
                    deleted_at = ?8,
                    sort_index = ?9,
                    icon = ?10,
                    icon_color = ?11
                 WHERE id = ?12",
                params![
                    project.name,
                    project.path,
                    project.description,
                    project.claude_provider_id,
                    project.created_at,
                    project.updated_at,
                    project.last_written_at,
                    project.deleted_at,
                    project.sort_index,
                    project.icon,
                    project.icon_color,
                    project.id,
                ],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        } else {
            tx.execute(
                "INSERT INTO projects (
                    id, name, path, description, claude_provider_id,
                    created_at, updated_at, last_written_at, deleted_at,
                    sort_index, icon, icon_color
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    project.id,
                    project.name,
                    project.path,
                    project.description,
                    project.claude_provider_id,
                    project.created_at,
                    project.updated_at,
                    project.last_written_at,
                    project.deleted_at,
                    project.sort_index,
                    project.icon,
                    project.icon_color,
                ],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }

        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 按 id 查询项目（含已软删的；是否过滤由 service 层决定）
    pub fn get_project(&self, id: &str) -> Result<Option<Project>, AppError> {
        let conn = lock_conn!(self.conn);
        let result = conn.query_row(
            "SELECT id, name, path, description, claude_provider_id,
                    created_at, updated_at, last_written_at, deleted_at,
                    sort_index, icon, icon_color
             FROM projects WHERE id = ?1",
            params![id],
            row_to_project,
        );
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 列出项目；include_deleted=false 时过滤软删。
    /// 排序：sort_index ASC（NULL 末尾）→ created_at ASC → id ASC
    pub fn list_projects(&self, include_deleted: bool) -> Result<Vec<Project>, AppError> {
        let conn = lock_conn!(self.conn);
        let sql = if include_deleted {
            "SELECT id, name, path, description, claude_provider_id,
                    created_at, updated_at, last_written_at, deleted_at,
                    sort_index, icon, icon_color
             FROM projects
             ORDER BY COALESCE(sort_index, 9999999999) ASC, created_at ASC, id ASC"
        } else {
            "SELECT id, name, path, description, claude_provider_id,
                    created_at, updated_at, last_written_at, deleted_at,
                    sort_index, icon, icon_color
             FROM projects
             WHERE deleted_at IS NULL
             ORDER BY COALESCE(sort_index, 9999999999) ASC, created_at ASC, id ASC"
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_project)
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(projects)
    }

    /// 软删除项目（设置 deleted_at；不物理删除）
    pub fn soft_delete_project(&self, id: &str, now_millis: i64) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE projects SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![now_millis, id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 恢复软删项目（清除 deleted_at）
    pub fn restore_project(&self, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE projects SET deleted_at = NULL WHERE id = ?1",
            params![id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 更新项目绑定的 Claude provider（同时更新 updated_at）
    pub fn update_project_provider(
        &self,
        id: &str,
        claude_provider_id: Option<&str>,
        now_millis: i64,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE projects SET claude_provider_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![claude_provider_id, now_millis, id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 更新项目上次成功写入 .claude/settings.json 的时间
    pub fn update_project_last_written_at(&self, id: &str, ts: i64) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE projects SET last_written_at = ?1 WHERE id = ?2",
            params![ts, id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 计算下一个可用的 sort_index（MAX(sort_index)+1，空表返回 0）
    pub fn next_project_sort_index(&self) -> Result<i64, AppError> {
        let conn = lock_conn!(self.conn);
        let max: Option<i64> = conn
            .query_row("SELECT MAX(sort_index) FROM projects", [], |row| row.get(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(max.map(|v| v + 1).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_project(id: &str, name: &str, path: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_string(),
            description: None,
            claude_provider_id: None,
            created_at: 1000,
            updated_at: 1000,
            last_written_at: None,
            deleted_at: None,
            sort_index: None,
            icon: None,
            icon_color: None,
        }
    }

    #[test]
    fn save_and_get_project_roundtrip() {
        let db = Database::memory().expect("memory db");
        let mut p = make_project("p1", "SLG", "/work/slg");
        p.description = Some("main".into());
        p.claude_provider_id = Some("packy".into());
        p.sort_index = Some(0);

        db.save_project(&p).expect("save");
        let got = db.get_project("p1").expect("get").expect("present");
        assert_eq!(got.id, "p1");
        assert_eq!(got.name, "SLG");
        assert_eq!(got.path, "/work/slg");
        assert_eq!(got.description.as_deref(), Some("main"));
        assert_eq!(got.claude_provider_id.as_deref(), Some("packy"));
        assert_eq!(got.sort_index, Some(0));
        assert!(got.deleted_at.is_none());
        assert!(got.last_written_at.is_none());
    }

    #[test]
    fn save_project_upserts_on_existing_id() {
        let db = Database::memory().expect("memory db");
        let mut p = make_project("p1", "Old", "/a");
        db.save_project(&p).expect("save old");
        p.name = "New".into();
        p.path = "/b".into();
        db.save_project(&p).expect("save new (upsert)");

        let got = db.get_project("p1").expect("get").expect("present");
        assert_eq!(got.name, "New");
        assert_eq!(got.path, "/b");
    }

    #[test]
    fn get_project_returns_none_for_missing() {
        let db = Database::memory().expect("memory db");
        assert!(db.get_project("nope").expect("get").is_none());
    }

    #[test]
    fn list_projects_excludes_soft_deleted_by_default() {
        let db = Database::memory().expect("memory db");
        let mut a = make_project("a", "A", "/a");
        a.sort_index = Some(0);
        let mut b = make_project("b", "B", "/b");
        b.sort_index = Some(1);
        db.save_project(&a).expect("save a");
        db.save_project(&b).expect("save b");
        db.soft_delete_project("b", 5000).expect("soft delete b");

        let active = db.list_projects(false).expect("list");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "a");

        let all = db.list_projects(true).expect("list all");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn soft_delete_then_restore() {
        let db = Database::memory().expect("memory db");
        db.save_project(&make_project("a", "A", "/a"))
            .expect("save");

        db.soft_delete_project("a", 5000).expect("delete");
        let active = db.list_projects(false).expect("list");
        assert!(active.is_empty(), "soft-deleted not in active list");
        let got = db.get_project("a").expect("get").expect("present");
        assert_eq!(got.deleted_at, Some(5000));

        db.restore_project("a").expect("restore");
        let active = db.list_projects(false).expect("list");
        assert_eq!(active.len(), 1);
        let got = db.get_project("a").expect("get").expect("present");
        assert!(got.deleted_at.is_none());
    }

    #[test]
    fn update_project_provider_sets_provider_and_updated_at() {
        let db = Database::memory().expect("memory db");
        db.save_project(&make_project("a", "A", "/a"))
            .expect("save");

        db.update_project_provider("a", Some("glm4"), 9999)
            .expect("update");
        let got = db.get_project("a").expect("get").expect("present");
        assert_eq!(got.claude_provider_id.as_deref(), Some("glm4"));
        assert_eq!(got.updated_at, 9999);

        db.update_project_provider("a", None, 1111).expect("clear");
        let got = db.get_project("a").expect("get").expect("present");
        assert!(got.claude_provider_id.is_none());
        assert_eq!(got.updated_at, 1111);
    }

    #[test]
    fn update_project_last_written_at() {
        let db = Database::memory().expect("memory db");
        db.save_project(&make_project("a", "A", "/a"))
            .expect("save");
        db.update_project_last_written_at("a", 5555)
            .expect("update");

        let got = db.get_project("a").expect("get").expect("present");
        assert_eq!(got.last_written_at, Some(5555));
    }

    #[test]
    fn next_project_sort_index_empty_then_incrementing() {
        let db = Database::memory().expect("memory db");
        assert_eq!(db.next_project_sort_index().expect("next"), 0);

        let mut a = make_project("a", "A", "/a");
        a.sort_index = Some(0);
        let mut b = make_project("b", "B", "/b");
        b.sort_index = Some(1);
        db.save_project(&a).expect("save a");
        db.save_project(&b).expect("save b");

        assert_eq!(db.next_project_sort_index().expect("next"), 2);
    }

    #[test]
    fn list_projects_orders_by_sort_index_then_created_at() {
        let db = Database::memory().expect("memory db");
        // 故意打乱插入顺序
        let mut c = make_project("c", "C", "/c");
        c.sort_index = Some(2);
        c.created_at = 100;
        let mut a = make_project("a", "A", "/a");
        a.sort_index = Some(0);
        a.created_at = 300;
        let mut b = make_project("b", "B", "/b");
        b.sort_index = Some(0); // 同 sort_index，按 created_at 升序
        b.created_at = 200;
        db.save_project(&c).expect("save c");
        db.save_project(&a).expect("save a");
        db.save_project(&b).expect("save b");

        let list = db.list_projects(false).expect("list");
        let ids: Vec<_> = list.iter().map(|p| p.id.as_str()).collect();
        // sort_index 0 在前；同 0 按 created_at 升序（b=200 < a=300）；然后 c(2)
        assert_eq!(ids, vec!["b", "a", "c"]);
    }
}
