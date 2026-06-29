//! 项目工程目录管理 Tauri command
//!
//! 前端 IPC 入口，薄包装 [`ProjectService`]。返回 `Result<T, String>`（前端友好），
//! `AppError` 通过 `to_string()` 转换。
//!
//! `write_project_claude_settings`（M2）和 `open_project_terminal`（M3）当前为占位，
//! 待对应里程碑填充。

use tauri::State;

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

/// 在项目目录打开终端并执行命令（默认 `claude`，或用户自定义命令）。
///
/// **跨平台直接启动终端在项目目录**，不依赖 `launch_terminal_running`（它的 `start`
/// 不传 `/D`，新窗口起始目录是调用者 cwd）。claude 读项目根 `settings.local.json`
/// 获取 provider 配置。
#[tauri::command]
pub async fn open_project_terminal(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] projectId: String,
    #[allow(non_snake_case)] customCommand: Option<String>,
) -> Result<bool, String> {
    let project = ProjectService::get(&state.db, &projectId)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("项目 {projectId} 不存在"))?;

    let cmd = customCommand
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "claude".to_string());

    let project_path = &project.path;
    log::info!(
        "[open_project_terminal] project='{}' path='{}' cmd='{}'",
        project.name,
        project_path,
        cmd
    );

    // Windows: 用 cmd /C "完整命令行" 让 cmd 自己解析（避免 args 数组的转义问题）
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        // start 第一个参数是窗口标题（必须），/D 是起始目录，cmd /K 保持窗口
        // 用嵌入引号的单字符串让 cmd 解析
        let full_cmd = format!(
            "start \"CC-Switch\" /D \"{}\" cmd /K \"{}\"",
            project_path, cmd
        );
        log::info!("[open_project_terminal] full_cmd: {full_cmd}");
        let result = Command::new("cmd").args(["/C", &full_cmd]).spawn();
        match result {
            Ok(_) => {
                log::info!("[open_project_terminal] Windows cmd 启动成功，cwd={project_path}");
                Ok(true)
            }
            Err(e) => Err(format!("启动终端失败: {e}")),
        }
    }

    // macOS: osascript 打开 Terminal 在项目目录执行命令
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\"\n\
             activate\n\
             do script \"cd '{path}' && {cmd}\"\n\
             end tell",
            path = project_path.replace('\'', "'\\''"),
            cmd = cmd.replace('\'', "'\\''"),
        );
        let result = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
        match result {
            Ok(_) => Ok(true),
            Err(e) => Err(format!("启动 Terminal 失败: {e}")),
        }
    }

    // Linux: 写临时脚本 + gnome-terminal --working-directory
    #[cfg(target_os = "linux")]
    {
        let temp_dir = std::env::temp_dir();
        let script_file = temp_dir.join(format!("ccs_project_{}.sh", std::process::id()));
        let content = format!(
            "#!/usr/bin/env sh\ncd \"{}\" && {}\nread -r _\n",
            project_path, cmd
        );
        std::fs::write(&script_file, &content).map_err(|e| format!("写入脚本失败: {e}"))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("设置权限失败: {e}"))?;
        let result = std::process::Command::new("gnome-terminal")
            .args(["--working-directory", project_path])
            .args(["--", "sh", script_file.to_string_lossy().as_ref()])
            .spawn();
        match result {
            Ok(_) => Ok(true),
            Err(e) => Err(format!("启动终端失败: {e}")),
        }
    }
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
