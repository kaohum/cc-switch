import { useEffect, useState } from "react";
import { providersApi, type AppId } from "@/lib/api";
import type { Provider } from "@/types";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useTranslation } from "react-i18next";

interface Props {
  value?: string;
  onChange: (providerId: string | null) => void;
  disabled?: boolean;
}

const NONE = "__none__";

/** 选择绑定的 Claude provider（从全局 claude provider 池） */
export function ProviderSelectForProject({ value, onChange, disabled }: Props) {
  const { t } = useTranslation();
  const [providers, setProviders] = useState<Record<string, Provider>>({});

  useEffect(() => {
    providersApi
      .getAll("claude" as AppId)
      .then(setProviders)
      .catch(() => {
        /* 忽略，UI 显示空列表 */
      });
  }, []);

  return (
    <Select
      value={value ?? NONE}
      disabled={disabled}
      onValueChange={(v) => onChange(v === NONE ? null : v)}
    >
      <SelectTrigger>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={NONE}>{t("projects.noProvider")}</SelectItem>
        {Object.entries(providers).map(([id, p]) => (
          <SelectItem key={id} value={id}>
            {p.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
