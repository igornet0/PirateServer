import { FolderOpen, Globe, LayoutDashboard, Link2 } from "lucide-react";
import React from "react";
import { useI18n } from "./i18n";

export type MainTab = "overview" | "projects" | "connection" | "internet";

const navBtn =
  "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm font-medium transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/60";

export function SidebarNav({
  mainTab,
  onTab,
}: {
  mainTab: MainTab;
  onTab: (t: MainTab) => void;
}) {
  const { t } = useI18n();
  /** Order matches default tab (`projects`) to reduce navigation/entry mismatch. */
  const items: { id: MainTab; label: string; icon: React.ReactNode }[] = [
    { id: "projects", label: t("sidebar.projects"), icon: <FolderOpen className="h-4 w-4 shrink-0 text-red-400/90" /> },
    { id: "overview", label: t("sidebar.overview"), icon: <LayoutDashboard className="h-4 w-4 shrink-0 text-red-400/90" /> },
    { id: "connection", label: t("sidebar.connection"), icon: <Link2 className="h-4 w-4 shrink-0 text-red-400/90" /> },
    { id: "internet", label: t("sidebar.internet"), icon: <Globe className="h-4 w-4 shrink-0 text-red-400/90" /> },
  ];

  return (
    <nav className="flex flex-col gap-0.5 p-2" aria-label={t("sidebar.sections")}>
      {items.map(({ id, label, icon }) => {
        const active = mainTab === id;
        return (
          <button
            key={id}
            type="button"
            onClick={() => onTab(id)}
            className={`${navBtn} ${
              active
                ? "border-l-2 border-red-600 bg-red-950/35 text-red-50 shadow-glow"
                : "border-l-2 border-transparent text-slate-400 hover:bg-red-950/20 hover:text-red-100"
            }`}
          >
            {icon}
            {label}
          </button>
        );
      })}
    </nav>
  );
}
