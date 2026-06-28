import { useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Label } from "@/components/ui/label";
import { useTranslation } from "react-i18next";
import { ProviderSelectForProject } from "./ProviderSelectForProject";
import type { Project } from "@/types/project";

export interface ProjectFormData {
  name: string;
  path: string;
  description?: string;
  claudeProviderId?: string;
}

interface Props {
  open: boolean;
  /** 传入 = 编辑；null/undefined = 新建 */
  project?: Project | null;
  onSubmit: (data: ProjectFormData) => void;
  onCancel: () => void;
}

export function ProjectFormDialog({
  open,
  project,
  onSubmit,
  onCancel,
}: Props) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [description, setDescription] = useState("");
  const [providerId, setProviderId] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setName(project?.name ?? "");
      setPath(project?.path ?? "");
      setDescription(project?.description ?? "");
      setProviderId(project?.claudeProviderId ?? null);
    }
  }, [open, project]);

  const canSubmit = name.trim().length > 0 && path.trim().length > 0;

  const submit = () => {
    if (!canSubmit) return;
    onSubmit({
      name: name.trim(),
      path: path.trim(),
      description: description.trim() || undefined,
      claudeProviderId: providerId ?? undefined,
    });
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onCancel()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {project ? t("projects.edit") : t("projects.create")}
          </DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-1.5">
            <Label>{t("projects.name")}</Label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("projects.namePlaceholder")}
            />
          </div>
          <div className="space-y-1.5">
            <Label>{t("projects.path")}</Label>
            <Input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="/path/to/project"
              className="font-mono text-xs"
            />
          </div>
          <div className="space-y-1.5">
            <Label>{t("projects.description")}</Label>
            <Textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={2}
            />
          </div>
          <div className="space-y-1.5">
            <Label>{t("projects.claudeProvider")}</Label>
            <ProviderSelectForProject
              value={providerId ?? undefined}
              onChange={setProviderId}
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button onClick={submit} disabled={!canSubmit}>
            {t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
