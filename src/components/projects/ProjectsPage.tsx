import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Copy,
  Edit,
  FolderOpen,
  Plus,
  RefreshCw,
  Terminal,
  Trash2,
} from "lucide-react";
import { useProjects } from "@/hooks/useProjects";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Input } from "@/components/ui/input";
import { ProviderSelectForProject } from "./ProviderSelectForProject";
import { ProjectFormDialog, type ProjectFormData } from "./ProjectFormDialog";
import type { Project } from "@/types/project";

export function ProjectsPage() {
  const { t } = useTranslation();
  const {
    projects,
    loading,
    reload,
    createProject,
    updateProject,
    deleteProject,
    setProvider,
    writeSettings,
    openTerminal,
    copyLaunchCommand,
  } = useProjects();

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<Project | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Project | null>(null);
  const [customCommand, setCustomCommand] = useState("");

  useEffect(() => {
    reload();
  }, [reload]);

  // 从 localStorage 读取待打开的项目 ID（ProviderCard 标签点击跳转）
  useEffect(() => {
    const pendingId = localStorage.getItem("ccs-open-project-id");
    if (pendingId) {
      localStorage.removeItem("ccs-open-project-id");
      setSelectedId(pendingId);
    }
  }, []);

  const selected =
    projects.find((p) => p.id === selectedId) ?? projects[0] ?? null;

  // 自定义命令持久化（per-project，设备级 localStorage）
  useEffect(() => {
    if (selected) {
      setCustomCommand(localStorage.getItem(`ccs-cmd-${selected.id}`) ?? "");
    }
  }, [selected?.id]);

  const handleCreate = () => {
    setEditing(null);
    setFormOpen(true);
  };
  const handleEdit = (p: Project) => {
    setEditing(p);
    setFormOpen(true);
  };

  const submitForm = async (data: ProjectFormData) => {
    if (editing) {
      const updated = await updateProject(editing.id, {
        name: data.name,
        path: data.path,
        description: data.description,
      });
      // 编辑时也允许改 provider（若变化）
      if (updated && data.claudeProviderId !== editing.claudeProviderId) {
        await setProvider(editing.id, data.claudeProviderId ?? null);
      }
    } else {
      const created = await createProject({
        name: data.name,
        path: data.path,
        description: data.description,
        claudeProviderId: data.claudeProviderId,
      });
      if (created) setSelectedId(created.id);
    }
    setFormOpen(false);
  };

  return (
    <div className="flex h-full overflow-hidden">
      {/* 左侧项目列表 */}
      <aside className="flex w-64 flex-col border-r">
        <div className="border-b p-2">
          <Button onClick={handleCreate} className="w-full" size="sm">
            <Plus className="h-4 w-4" /> {t("projects.create")}
          </Button>
        </div>
        <div className="flex-1 overflow-auto">
          {loading && projects.length === 0 ? (
            <div className="p-4 text-sm text-muted-foreground">
              {t("common.loading")}
            </div>
          ) : projects.length === 0 ? (
            <div className="p-4 text-sm text-muted-foreground">
              {t("projects.empty")}
            </div>
          ) : (
            projects.map((p) => (
              <button
                key={p.id}
                onClick={() => setSelectedId(p.id)}
                className={`block w-full px-3 py-2 text-left text-sm hover:bg-accent ${
                  selected?.id === p.id ? "bg-accent" : ""
                }`}
              >
                <div className="truncate font-medium">{p.name}</div>
                <div className="truncate text-xs text-muted-foreground">
                  {p.path}
                </div>
              </button>
            ))
          )}
        </div>
      </aside>

      {/* 右侧详情 */}
      <section className="flex-1 overflow-auto p-4">
        {!selected ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            <FolderOpen className="mr-2 h-5 w-5" />
            {t("projects.selectPrompt")}
          </div>
        ) : (
          <div className="mx-auto max-w-2xl space-y-5">
            <div className="flex items-center gap-2">
              <h2 className="flex-1 text-lg font-semibold">{selected.name}</h2>
              <Button
                size="sm"
                variant="outline"
                onClick={() => handleEdit(selected)}
              >
                <Edit className="h-4 w-4" /> {t("common.edit")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => setDeleteTarget(selected)}
              >
                <Trash2 className="h-4 w-4" /> {t("common.delete")}
              </Button>
            </div>

            <div className="space-y-1">
              <div className="text-xs text-muted-foreground">
                {t("projects.path")}
              </div>
              <div className="break-all font-mono text-sm">{selected.path}</div>
            </div>

            {selected.description ? (
              <div className="text-sm text-muted-foreground">
                {selected.description}
              </div>
            ) : null}

            <div className="space-y-2 border-t pt-4">
              <div className="text-sm font-medium">
                {t("projects.claudeProvider")}
              </div>
              <ProviderSelectForProject
                value={selected.claudeProviderId}
                onChange={(pid) => setProvider(selected.id, pid)}
              />
              <div className="text-xs text-muted-foreground">
                {selected.lastWrittenAt
                  ? `${t("projects.lastWritten")}: ${new Date(
                      selected.lastWrittenAt,
                    ).toLocaleString()}`
                  : t("projects.notWrittenYet")}
              </div>
              <div className="text-xs text-muted-foreground">
                {t("projects.writeTarget")}:{" "}
                <span className="font-mono">
                  {selected.path}/.claude/settings.local.json
                </span>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 border-t pt-4">
              <Input
                placeholder={t("projects.customCommandPlaceholder", {
                  defaultValue: "自定义命令（留空=claude）",
                })}
                value={customCommand}
                onChange={(e) => {
                  setCustomCommand(e.target.value);
                  if (selected) {
                    localStorage.setItem(
                      `ccs-cmd-${selected.id}`,
                      e.target.value,
                    );
                  }
                }}
                className="h-8 max-w-xs font-mono text-xs"
              />
              <Button
                size="sm"
                variant="outline"
                onClick={() =>
                  openTerminal(selected.id, customCommand.trim() || undefined)
                }
              >
                <Terminal className="h-4 w-4" /> {t("projects.openTerminal")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => copyLaunchCommand(selected.id)}
              >
                <Copy className="h-4 w-4" /> {t("projects.copyCommand")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => writeSettings(selected.id)}
              >
                <RefreshCw className="h-4 w-4" /> {t("projects.rewrite")}
              </Button>
            </div>
          </div>
        )}
      </section>

      <ProjectFormDialog
        open={formOpen}
        project={editing}
        onSubmit={submitForm}
        onCancel={() => setFormOpen(false)}
      />

      <ConfirmDialog
        isOpen={!!deleteTarget}
        title={t("projects.deleteTitle")}
        message={t("projects.deleteConfirm", {
          name: deleteTarget?.name ?? "",
        })}
        onConfirm={async () => {
          if (deleteTarget) {
            const ok = await deleteProject(deleteTarget.id);
            if (ok && selectedId === deleteTarget.id) setSelectedId(null);
          }
          setDeleteTarget(null);
        }}
        onCancel={() => setDeleteTarget(null)}
      />
    </div>
  );
}
