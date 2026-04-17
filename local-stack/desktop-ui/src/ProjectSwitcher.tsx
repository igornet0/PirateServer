import React from "react";
import { RegisteredProjectsList } from "./RegisteredProjectsList";
import { useI18n } from "./i18n";

/** Compact project list for the left sidebar (same data as full registry view). */
export function ProjectSwitcher(props: {
  refreshKey: number;
  currentDeployDir: string | null;
  onSelectPath: (path: string) => void;
  onRegistryChanged?: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex min-h-0 flex-1 flex-col border-t border-border-subtle">
      <p className="px-3 py-2 font-display text-xs tracking-wide text-red-400/90">
        {t("switcher.title")}
      </p>
      <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-2">
        <RegisteredProjectsList {...props} variant="compact" />
      </div>
    </div>
  );
}
