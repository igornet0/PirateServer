import { ChevronDown, ChevronRight } from "lucide-react";
import React, { useMemo, useState } from "react";
import { useI18n } from "../../i18n";
import type { WorkspaceTab } from "./hostDbWorkspaceStore";

type Props = {
  tab: WorkspaceTab | null;
  fkJson: string | null;
  onOpenAdmin: () => void;
  contextTab: "properties" | "indexes" | "grants";
  onContextTab: (t: "properties" | "indexes" | "grants") => void;
};

export function DbContextPanel({ tab, fkJson, onOpenAdmin, contextTab, onContextTab }: Props) {
  const { t } = useI18n();
  const [fkOpen, setFkOpen] = useState(false);

  const columns = useMemo(() => {
    if (!tab) return [];
    if (tab.kind === "table_data") return tab.data.columns;
    if (tab.kind === "table_schema") return tab.data.columns;
    return [];
  }, [tab]);

  const schemaTable =
    tab && (tab.kind === "table_data" || tab.kind === "table_schema")
      ? `${tab.schema}.${tab.table}`
      : null;

  return (
    <div className="flex min-h-0 min-w-[11rem] max-w-[16rem] flex-1 flex-col border-l border-red-900/20 bg-black/25">
      <div className="shrink-0 border-b border-red-900/20 px-2 py-1.5 text-[10px] font-semibold uppercase tracking-wide text-slate-500">
        {t("db.workspace.context")}
      </div>
      <div className="flex shrink-0 gap-0.5 border-b border-white/5 px-1 py-1">
        {(
          [
            ["properties", t("db.workspace.contextProps")] as const,
            ["indexes", t("db.workspace.contextIdx")] as const,
            ["grants", t("db.workspace.contextGrants")] as const,
          ]
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            onClick={() => onContextTab(id)}
            className={`rounded px-1.5 py-0.5 text-[9px] ${
              contextTab === id ? "bg-red-950/50 text-amber-100" : "text-slate-500 hover:text-slate-300"
            }`}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2 text-[10px] text-slate-400">
        {!tab ? <p className="text-slate-600">{t("db.workspace.contextEmpty")}</p> : null}

        {tab?.kind === "sql" ? (
          <p className="leading-relaxed text-slate-500">{t("db.workspace.contextSqlHint")}</p>
        ) : null}

        {tab?.kind === "admin" ? (
          <button
            type="button"
            onClick={onOpenAdmin}
            className="mt-1 text-violet-300/90 underline decoration-dotted"
          >
            {t("db.workspace.contextAdminFocus")}
          </button>
        ) : null}

        {contextTab === "properties" && schemaTable ? (
          <div className="mb-2 font-mono text-[10px] text-amber-200/80">{schemaTable}</div>
        ) : null}

        {contextTab === "properties" && columns.length > 0 ? (
          <ul className="space-y-1">
            {columns.map((c) => (
              <li key={c.name} className="flex flex-col rounded border border-white/5 bg-black/20 px-1.5 py-1">
                <span className="font-mono text-slate-200">{c.name}</span>
                <span className="text-[9px] text-slate-600">{c.type}</span>
              </li>
            ))}
          </ul>
        ) : null}

        {contextTab === "properties" && tab && columns.length === 0 ? (
          <p className="text-slate-600">{t("db.workspace.noColumns")}</p>
        ) : null}

        {contextTab === "indexes" ? (
          <p className="leading-relaxed text-slate-600">{t("db.workspace.indexesPlaceholder")}</p>
        ) : null}

        {contextTab === "grants" ? (
          <div className="space-y-2">
            <p className="leading-relaxed text-slate-600">{t("db.workspace.grantsHint")}</p>
            {fkJson ? (
              <div>
                <button
                  type="button"
                  className="flex w-full items-center gap-1 text-left text-[10px] text-slate-300 hover:text-amber-200/90"
                  onClick={() => setFkOpen((o) => !o)}
                >
                  {fkOpen ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
                  {t("db.fk")}
                </button>
                {fkOpen ? (
                  <pre className="mt-1 max-h-48 overflow-auto rounded border border-border-subtle bg-black/40 p-2 font-mono text-[9px] text-slate-500">
                    {fkJson.length > 4000 ? `${fkJson.slice(0, 4000)}…` : fkJson}
                  </pre>
                ) : null}
              </div>
            ) : (
              <p className="text-[9px] text-slate-600">{t("db.workspace.fkLoadInSchema")}</p>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}
