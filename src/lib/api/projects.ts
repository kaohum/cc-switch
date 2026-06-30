// 项目工程目录管理 API
import { invoke } from "@tauri-apps/api/core";
import type {
  CreateProjectRequest,
  Project,
  ProjectPathValidation,
  SetProviderResult,
  UpdateProjectRequest,
} from "@/types/project";

/** 列出项目；includeDeleted=false（默认）时排除软删项目 */
export async function listProjects(includeDeleted = false): Promise<Project[]> {
  return invoke<Project[]>("list_projects", { includeDeleted });
}

export async function getProject(id: string): Promise<Project | null> {
  return invoke<Project | null>("get_project", { id });
}

export async function createProject(
  request: CreateProjectRequest,
): Promise<Project> {
  return invoke<Project>("create_project", { request });
}

export async function updateProject(
  id: string,
  request: UpdateProjectRequest,
): Promise<Project> {
  return invoke<Project>("update_project", { id, request });
}

/** 软删除项目 */
export async function deleteProject(id: string): Promise<boolean> {
  return invoke<boolean>("delete_project", { id });
}

export async function restoreProject(id: string): Promise<boolean> {
  return invoke<boolean>("restore_project", { id });
}

/**
 * 设置项目绑定的 Claude provider。
 * 绑定/解绑后后端 best-effort 同步写入项目根 .claude/settings.local.json：
 * 写盘失败时 result.writeWarning 有值（如项目目录不存在），绑定本身仍成功。
 */
export async function setProjectClaudeProvider(
  projectId: string,
  providerId: string | null,
): Promise<SetProviderResult> {
  return invoke<SetProviderResult>("set_project_claude_provider", {
    projectId,
    providerId,
  });
}

/** 手动重新写入项目根 .claude/settings.json，返回写入路径 */
export async function writeProjectClaudeSettings(
  projectId: string,
): Promise<string> {
  return invoke<string>("write_project_claude_settings", { projectId });
}

/** 在项目目录打开终端并启动 claude（用项目绑定的 provider），或跑自定义命令 */
export async function openProjectTerminal(
  projectId: string,
  customCommand?: string,
): Promise<boolean> {
  return invoke<boolean>("open_project_terminal", { projectId, customCommand });
}

/** 返回在项目目录启动 claude 的命令字符串（cd "<path>" && claude） */
export async function copyProjectLaunchCommand(
  projectId: string,
): Promise<string> {
  return invoke<string>("copy_project_launch_command", { projectId });
}

/** 校验项目路径状态（用于 UI 提示） */
export async function validateProjectPath(
  path: string,
): Promise<ProjectPathValidation> {
  return invoke<ProjectPathValidation>("validate_project_path", { path });
}
