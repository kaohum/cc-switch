import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import * as projectsApi from "@/lib/api/projects";
import type {
  CreateProjectRequest,
  Project,
  ProjectPathValidation,
  UpdateProjectRequest,
} from "@/types/project";

/**
 * 项目工程目录管理 hook（state + actions）。
 * 跟随 usePromptActions 风格：useState + useCallback + sonner toast。
 * action 返回值用于组件判断成功与否（null/false = 失败，错误已 toast）。
 */
export function useProjects() {
  const { t } = useTranslation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(false);

  const reload = useCallback(
    async (includeDeleted = false) => {
      setLoading(true);
      try {
        const data = await projectsApi.listProjects(includeDeleted);
        setProjects(data);
        return data;
      } catch (error) {
        toast.error(t("projects.loadFailed"));
        return null;
      } finally {
        setLoading(false);
      }
    },
    [t],
  );

  const createProject = useCallback(
    async (req: CreateProjectRequest): Promise<Project | null> => {
      try {
        const p = await projectsApi.createProject(req);
        await reload();
        toast.success(t("projects.createSuccess"));
        return p;
      } catch (error) {
        toast.error(String(error));
        return null;
      }
    },
    [reload, t],
  );

  const updateProject = useCallback(
    async (id: string, req: UpdateProjectRequest): Promise<Project | null> => {
      try {
        const p = await projectsApi.updateProject(id, req);
        await reload();
        toast.success(t("projects.updateSuccess"));
        return p;
      } catch (error) {
        toast.error(String(error));
        return null;
      }
    },
    [reload, t],
  );

  const deleteProject = useCallback(
    async (id: string): Promise<boolean> => {
      try {
        await projectsApi.deleteProject(id);
        await reload();
        toast.success(t("projects.deleteSuccess"));
        return true;
      } catch (error) {
        toast.error(String(error));
        return false;
      }
    },
    [reload, t],
  );

  const restoreProject = useCallback(
    async (id: string): Promise<boolean> => {
      try {
        await projectsApi.restoreProject(id);
        await reload();
        toast.success(t("projects.restoreSuccess"));
        return true;
      } catch (error) {
        toast.error(String(error));
        return false;
      }
    },
    [reload, t],
  );

  /** 设置项目绑定的 Claude provider（后端会 best-effort 写入项目根 settings.json） */
  const setProvider = useCallback(
    async (id: string, providerId: string | null): Promise<Project | null> => {
      try {
        const p = await projectsApi.setProjectClaudeProvider(id, providerId);
        await reload();
        toast.success(t("projects.providerSetSuccess"));
        return p;
      } catch (error) {
        toast.error(String(error));
        return null;
      }
    },
    [reload, t],
  );

  /** 手动重新写入项目根 .claude/settings.json，返回写入路径 */
  const writeSettings = useCallback(
    async (id: string): Promise<string | null> => {
      try {
        const path = await projectsApi.writeProjectClaudeSettings(id);
        await reload();
        toast.success(t("projects.writeSuccess"));
        return path;
      } catch (error) {
        toast.error(String(error));
        return null;
      }
    },
    [reload, t],
  );

  const openTerminal = useCallback(
    async (id: string, customCommand?: string): Promise<boolean> => {
      try {
        await projectsApi.openProjectTerminal(id, customCommand);
        return true;
      } catch (error) {
        toast.error(String(error));
        return false;
      }
    },
    [t],
  );

  const copyLaunchCommand = useCallback(
    async (id: string): Promise<string | null> => {
      try {
        const cmd = await projectsApi.copyProjectLaunchCommand(id);
        await navigator.clipboard.writeText(cmd);
        toast.success(t("projects.copyCommandSuccess"));
        return cmd;
      } catch (error) {
        toast.error(String(error));
        return null;
      }
    },
    [t],
  );

  const validatePath = useCallback(
    async (path: string): Promise<ProjectPathValidation | null> => {
      try {
        return await projectsApi.validateProjectPath(path);
      } catch (error) {
        toast.error(String(error));
        return null;
      }
    },
    [t],
  );

  return {
    projects,
    loading,
    reload,
    createProject,
    updateProject,
    deleteProject,
    restoreProject,
    setProvider,
    writeSettings,
    openTerminal,
    copyLaunchCommand,
    validatePath,
  };
}
