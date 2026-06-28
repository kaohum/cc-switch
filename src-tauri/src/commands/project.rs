//! 项目工程目录管理 Tauri command
//!
//! 前端 IPC 入口，薄包装 [`ProjectService`]。返回 `Result<T, String>`（前端友好），
//! `AppError` 通过 `to_string()` 转换。
//!
//! `write_project_claude_settings`（M2）和 `open_project_terminal`（M3）当前为占位，
//! 待对应里程碑填充。

use tauri::State;

use crate::app_config::AppType;
use crate::database::Project;
use crate::services::{CreateProjectRequest, ProjectService, UpdateProjectRequest};
use crate::store::AppState;

#[tauri::command]
pub fn list_projects(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] includeDeleted: Option<bool>,
) -> Result<Vec<Project>, String> {
    ProjectService::list(&state.db, includeDeleted.unwrap_or(false)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_project(state: State<'_, AppState>, id: String) -> Result<Option<Project>, String> {
    ProjectService::get(&state.db, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_project(
    state: State<'_, AppState>,
    request: CreateProjectRequest,
) -> Result<Project, String> {
    ProjectService::create(&state.db, request).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_project(
    state: State<'_, AppState>,
    id: String,
    request: UpdateProjectRequest,
) -> Result<Project, String> {
    ProjectService::update(&state.db, &id, request).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_project(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    ProjectService::delete(&state.db, &id)
        .map(|_| true)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restore_project(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    ProjectService::restore(&state.db, &id)
        .map(|_| true)
        .map_err(|e| e.to_string())
}

/// 设置项目绑定的 Claude provider（自动更新数据库；M2 起追加写入项目根）。
#[tauri::command]
pub fn set_project_claude_provider(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] projectId: String,
    #[allow(non_snake_case)] providerId: Option<String>,
) -> Result<Project, String> {
    ProjectService::set_claude_provider(&state.db, &projectId, providerId.as_deref())
        .map_err(|e| e.to_string())
}

/// 手动重新写入项目根 .claude/settings.json（写前备份 + 原子写）。
/// 返回实际写入的路径字符串。
#[tauri::command]
pub fn write_project_claude_settings(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] projectId: String,
) -> Result<String, String> {
    let path = ProjectService::write_claude_to_project(&state.db, &projectId)
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// 在项目目录打开终端并启动 claude（用项目绑定的 Claude provider）。
///
/// 复用 `open_provider_terminal` 的跨平台终端启动逻辑：cwd = 项目根，
/// 注入项目绑定 provider 的配置。这直接实现「每个项目 CLI 用不同 provider」——
/// 不同项目目录启动的 claude 进程天然读各自的 provider 配置。
/// （若只想打开空终端手动操作，用 `copy_project_launch_command` 复制启动命令。）
#[tauri::command]
pub async fn open_project_terminal(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] projectId: String,
) -> Result<bool, String> {
    let project = ProjectService::get(&state.db, &projectId)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("项目 {projectId} 不存在"))?;
    let provider_id = project
        .claude_provider_id
        .clone()
        .ok_or_else(|| "项目未绑定 Claude provider，无法启动".to_string())?;

    crate::commands::open_provider_terminal(
        state,
        AppType::Claude.as_str().to_string(),
        provider_id,
        Some(project.path.clone()),
    )
    .await
}

/// 返回在项目目录启动 claude 的命令字符串（供前端复制到剪贴板）。
#[tauri::command]
pub fn copy_project_launch_command(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] projectId: String,
) -> Result<String, String> {
    let project = ProjectService::get(&state.db, &projectId)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("项目 {projectId} 不存在"))?;
    // 路径用双引号包裹，兼容含空格/中文的路径
    Ok(format!("cd \"{}\" && claude", project.path))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathValidation {
    pub exists: bool,
    pub is_directory: bool,
    pub writable: bool,
    pub parent_exists: bool,
}

/// 校验项目路径状态（用于 UI 提示，宽松策略：不存在不阻止创建）。
#[tauri::command]
pub fn validate_project_path(path: String) -> Result<ProjectPathValidation, String> {
    let p = std::path::Path::new(&path);
    let exists = p.exists();
    let is_directory = exists && p.is_dir();
    let writable = if is_directory {
        let tmp = p.join(".ccs-write-test");
        let ok = std::fs::write(&tmp, b"").is_ok();
        if ok {
            let _ = std::fs::remove_file(&tmp);
        }
        ok
    } else {
        false
    };
    let parent_exists = p.parent().is_some_and(|par| par.exists());
    Ok(ProjectPathValidation {
        exists,
        is_directory,
        writable,
        parent_exists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_existing_writable_directory() {
        let dir = TempDir::new().expect("tmp");
        let path = dir.path().to_string_lossy().to_string();
        let v = validate_project_path(path).expect("validate");
        assert!(v.exists);
        assert!(v.is_directory);
        assert!(v.writable);
        assert!(v.parent_exists);
    }

    #[test]
    fn validate_nonexistent_path_reports_existing_parent() {
        let dir = TempDir::new().expect("tmp");
        let path = dir
            .path()
            .join("does-not-exist")
            .to_string_lossy()
            .to_string();
        let v = validate_project_path(path).expect("validate");
        assert!(!v.exists);
        assert!(!v.is_directory);
        assert!(!v.writable);
        assert!(v.parent_exists, "父目录存在时应报告 parent_exists=true");
    }

    #[test]
    fn validate_file_is_not_directory_and_not_writable() {
        let dir = TempDir::new().expect("tmp");
        let file = dir.path().join("not-a-dir.txt");
        std::fs::write(&file, b"x").expect("write file");
        let v = validate_project_path(file.to_string_lossy().to_string()).expect("validate");
        assert!(v.exists);
        assert!(!v.is_directory);
        assert!(!v.writable, "非目录路径 writable 应为 false");
    }
}
