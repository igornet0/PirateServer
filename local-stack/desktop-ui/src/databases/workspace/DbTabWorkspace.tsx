import Editor from "@monaco-editor/react";
import React, { useCallback, useEffect, useRef } from "react";
import { useI18n } from "../../i18n";
import { HostDatabaseRelationalInspector } from "../HostDatabaseRelationalInspector";
import { DbDataGrid } from "./DbDataGrid";
import {
  hostDbColumns,
  hostDbQuery,
  hostDbRelationshipsJson,
  hostDbRows,
  type DbCredsInvoke,
} from "./hostDbApi";
import { useHostDbWorkspaceStore, type WorkspaceTab } from "./hostDbWorkspaceStore";

type Props = {
  tab: WorkspaceTab | null;
  secondaryTab: WorkspaceTab | null;
  splitEnabled: boolean;
  instanceId: string;
  engine: string;
  dbCredsInvoke: DbCredsInvoke;
  canRunReadonlySql: boolean;
  focusSchemaForInspector: string;
};

export function DbTabWorkspace({
  tab,
  secondaryTab,
  splitEnabled,
  instanceId,
  engine,
  dbCredsInvoke,
  canRunReadonlySql,
  focusSchemaForInspector,
}: Props) {
  const { t, language } = useI18n();
  const {
    updateTableData,
    updateSchemaTab,
    updateSqlTab,
    openDrawer,
    livePollSec,
  } = useHostDbWorkspaceStore();

  const loadingMoreRef = useRef(false);

  const reloadTableData = useCallback(
    async (tb: Extract<WorkspaceTab, { kind: "table_data" }>, resetOffset: boolean) => {
      const { pageSize } = tb.data;
      const offset = resetOffset ? 0 : tb.data.offset;
      updateTableData(tb.id, { status: "loading", error: null });
      try {
        const columns = await hostDbColumns(instanceId, tb.schema, tb.table, dbCredsInvoke);
        const { rows } = await hostDbRows(instanceId, tb.schema, tb.table, pageSize, offset, dbCredsInvoke);
        updateTableData(tb.id, {
          status: "idle",
          error: null,
          columns,
          rows: resetOffset ? rows : [...tb.data.rows, ...rows],
          offset: resetOffset ? 0 : offset,
          truncated: rows.length >= pageSize,
          warn: null,
        });
      } catch (e) {
        updateTableData(tb.id, {
          status: "error",
          error: e instanceof Error ? e.message : String(e),
        });
      }
    },
    [instanceId, dbCredsInvoke, updateTableData],
  );

  const dataTabId = tab?.kind === "table_data" ? tab.id : null;

  useEffect(() => {
    if (!dataTabId) return;
    const cur = useHostDbWorkspaceStore
      .getState()
      .tabs.find((x): x is Extract<WorkspaceTab, { kind: "table_data" }> => x.kind === "table_data" && x.id === dataTabId);
    if (!cur) return;
    void reloadTableData(cur, true);
  }, [dataTabId, reloadTableData]);

  useEffect(() => {
    if (!dataTabId || livePollSec <= 0) return;
    const iv = window.setInterval(() => {
      const cur = useHostDbWorkspaceStore
        .getState()
        .tabs.find((x): x is Extract<WorkspaceTab, { kind: "table_data" }> => x.kind === "table_data" && x.id === dataTabId);
      if (cur) void reloadTableData(cur, true);
    }, livePollSec * 1000);
    return () => window.clearInterval(iv);
  }, [dataTabId, livePollSec, reloadTableData]);

  const schemaTabId = tab?.kind === "table_schema" ? tab.id : null;

  useEffect(() => {
    if (!schemaTabId) return;
    const schTab = useHostDbWorkspaceStore
      .getState()
      .tabs.find((x): x is Extract<WorkspaceTab, { kind: "table_schema" }> => x.kind === "table_schema" && x.id === schemaTabId);
    if (!schTab) return;
    let cancelled = false;
    (async () => {
      updateSchemaTab(schTab.id, { status: "loading", error: null });
      try {
        const columns = await hostDbColumns(instanceId, schTab.schema, schTab.table, dbCredsInvoke);
        if (cancelled) return;
        updateSchemaTab(schTab.id, { status: "idle", error: null, columns });
      } catch (e) {
        if (cancelled) return;
        updateSchemaTab(schTab.id, {
          status: "error",
          error: e instanceof Error ? e.message : String(e),
        });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [schemaTabId, instanceId, dbCredsInvoke, updateSchemaTab]);

  const loadMore = useCallback(
    (tb: Extract<WorkspaceTab, { kind: "table_data" }>) => {
      const latest = useHostDbWorkspaceStore
        .getState()
        .tabs.find((x): x is Extract<WorkspaceTab, { kind: "table_data" }> => x.kind === "table_data" && x.id === tb.id);
      if (!latest) return;
      if (latest.data.status === "loading" || loadingMoreRef.current) return;
      if (!latest.data.truncated) return;
      loadingMoreRef.current = true;
      const nextOff = latest.data.offset + latest.data.pageSize;
      updateTableData(latest.id, { offset: nextOff, status: "loading" });
      void (async () => {
        try {
          const { rows } = await hostDbRows(
            instanceId,
            latest.schema,
            latest.table,
            latest.data.pageSize,
            nextOff,
            dbCredsInvoke,
          );
          const after = useHostDbWorkspaceStore
            .getState()
            .tabs.find((x): x is Extract<WorkspaceTab, { kind: "table_data" }> => x.kind === "table_data" && x.id === tb.id);
          const baseRows = after?.kind === "table_data" ? after.data.rows : [];
          updateTableData(tb.id, {
            status: "idle",
            rows: [...baseRows, ...rows],
            offset: nextOff,
            truncated: rows.length >= latest.data.pageSize,
          });
        } catch (e) {
          updateTableData(tb.id, {
            status: "error",
            error: e instanceof Error ? e.message : String(e),
          });
        } finally {
          loadingMoreRef.current = false;
        }
      })();
    },
    [instanceId, dbCredsInvoke, updateTableData],
  );

  const runSqlTab = useCallback(
    async (tb: Extract<WorkspaceTab, { kind: "sql" }>) => {
      updateSqlTab(tb.id, { status: "loading", error: null });
      try {
        const result = await hostDbQuery(instanceId, tb.data.sql, 500, dbCredsInvoke);
        updateSqlTab(tb.id, { status: "idle", result, error: null });
      } catch (e) {
        updateSqlTab(tb.id, {
          status: "error",
          error: e instanceof Error ? e.message : String(e),
          result: null,
        });
      }
    },
    [instanceId, dbCredsInvoke, updateSqlTab],
  );

  const renderPane = (pane: WorkspaceTab | null) => {
    if (!pane) {
      return (
        <div className="flex min-h-0 flex-1 items-center justify-center text-[11px] text-slate-600">
          {t("db.workspace.pickTab")}
        </div>
      );
    }

    if (pane.kind === "table_data") {
      const tb = pane;
      const clientFiltered =
        tb.data.filterColumn && tb.data.filterValue
          ? tb.data.rows.filter((r) => {
              if (!r || typeof r !== "object") return false;
              const v = (r as Record<string, unknown>)[tb.data.filterColumn!];
              return String(v ?? "")
                .toLowerCase()
                .includes(tb.data.filterValue.toLowerCase());
            })
          : tb.data.rows;
      return (
        <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2 p-2">
          <div className="flex flex-wrap items-center gap-2 text-[10px] text-slate-500">
            <span className="font-mono text-slate-400">
              {tb.schema}.{tb.table}
            </span>
            {tb.data.status === "loading" ? <span>…</span> : null}
            {tb.data.error ? <span className="text-rose-300">{tb.data.error}</span> : null}
            <label className="flex items-center gap-1">
              {language === "ru" ? "Фильтр" : "Filter"}
              <select
                className="rounded border border-red-900/30 bg-black/40 px-1 py-0.5 font-mono text-[10px] text-slate-200"
                value={tb.data.filterColumn ?? ""}
                onChange={(e) =>
                  updateTableData(tb.id, { filterColumn: e.target.value || null })
                }
              >
                <option value="">—</option>
                {tb.data.columns.map((c) => (
                  <option key={c.name} value={c.name}>
                    {c.name}
                  </option>
                ))}
              </select>
              <input
                className="w-28 rounded border border-red-900/30 bg-black/40 px-1 py-0.5 font-mono text-[10px]"
                value={tb.data.filterValue}
                onChange={(e) => updateTableData(tb.id, { filterValue: e.target.value })}
                placeholder="…"
              />
            </label>
            <button
              type="button"
              className="rounded border border-border-subtle px-1.5 py-0.5 hover:bg-white/5"
              onClick={() => void reloadTableData(tb, true)}
            >
              {t("db.workspace.reloadData")}
            </button>
          </div>
          {tb.data.truncated ? (
            <p className="text-[10px] text-amber-200/70">{language === "ru" ? "Есть ещё строки — прокрутите вниз" : "More rows — scroll down"}</p>
          ) : null}
          {tb.data.warn ? <p className="text-[10px] text-amber-200/80">{tb.data.warn}</p> : null}
          <DbDataGrid
            columns={tb.data.columns}
            rows={clientFiltered}
            busy={tb.data.status === "loading"}
            onRowDetail={(i) => {
              const r = clientFiltered[i];
              openDrawer(
                `${tb.schema}.${tb.table} #${i + 1}`,
                typeof r === "object" ? JSON.stringify(r, null, 2) : String(r),
              );
            }}
            onNeedMore={
              tb.data.filterColumn && tb.data.filterValue ? undefined : () => loadMore(tb)
            }
          />
        </div>
      );
    }

    if (pane.kind === "table_schema") {
      const st = pane;
      return (
        <div className="min-h-0 flex-1 space-y-2 overflow-auto p-2 text-[11px]">
          <div className="font-mono text-slate-300">
            {st.schema}.{st.table}
          </div>
          {st.data.status === "loading" ? <p className="text-slate-500">…</p> : null}
          {st.data.error ? <p className="text-rose-300">{st.data.error}</p> : null}
          <ul className="space-y-1">
            {st.data.columns.map((c) => (
              <li key={c.name} className="rounded border border-white/5 bg-black/20 px-2 py-1 font-mono text-[10px]">
                <span className="text-slate-200">{c.name}</span>{" "}
                <span className="text-slate-600">{c.type}</span>
              </li>
            ))}
          </ul>
          <div className="flex flex-wrap gap-2 border-t border-white/5 pt-2">
            <button
              type="button"
              disabled={st.data.fkStatus === "loading"}
              className="rounded border border-border-subtle bg-black/30 px-2 py-1 text-[10px] hover:bg-black/45"
              onClick={() => {
                updateSchemaTab(st.id, { fkStatus: "loading", fkError: null });
                void (async () => {
                  try {
                    const j = await hostDbRelationshipsJson(instanceId, dbCredsInvoke);
                    updateSchemaTab(st.id, { fkJson: j, fkStatus: "idle", fkError: null });
                  } catch (e) {
                    updateSchemaTab(st.id, {
                      fkStatus: "error",
                      fkError: e instanceof Error ? e.message : String(e),
                    });
                  }
                })();
              }}
            >
              {t("db.fk")}
            </button>
          </div>
          {st.data.fkError ? <p className="text-[10px] text-rose-300">{st.data.fkError}</p> : null}
          {st.data.fkJson ? (
            <pre className="max-h-64 overflow-auto rounded border border-border-subtle bg-black/40 p-2 text-[10px] text-slate-500">
              {st.data.fkJson.length > 6000 ? `${st.data.fkJson.slice(0, 6000)}…` : st.data.fkJson}
            </pre>
          ) : null}
        </div>
      );
    }

    if (pane.kind === "sql") {
      const sq = pane;
      return (
        <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2 p-2">
          <div className="min-h-[8rem] shrink-0 overflow-hidden rounded border border-red-900/25">
            <Editor
              height="8rem"
              theme="vs-dark"
              defaultLanguage="sql"
              value={sq.data.sql}
              onChange={(v) => updateSqlTab(sq.id, { sql: v ?? "" })}
              options={{ minimap: { enabled: false }, fontSize: 11, wordWrap: "on" }}
            />
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="rounded border border-amber-800/50 bg-amber-950/35 px-2 py-1 text-[11px] text-amber-100 disabled:opacity-50"
              disabled={sq.data.status === "loading"}
              onClick={() => void runSqlTab(sq)}
            >
              {t("db.run")}
            </button>
            <span className="text-[10px] text-slate-600">⌘/Ctrl+Enter</span>
          </div>
          {sq.data.error ? <p className="text-[10px] text-rose-300">{sq.data.error}</p> : null}
          {sq.data.result &&
          (sq.data.result.columns.length > 0 || (sq.data.result.rows && sq.data.result.rows.length > 0)) ? (
            <DbDataGrid
              columns={sq.data.result.columns.map((c) => ({ name: c }))}
              rows={sq.data.result.rows as unknown[]}
              onRowDetail={(i) => {
                const r = sq.data.result?.rows[i];
                openDrawer(`SQL row #${i + 1}`, JSON.stringify(r, null, 2));
              }}
            />
          ) : null}
          {sq.data.result?.warn ? (
            <p className="text-[10px] text-amber-200/80">{sq.data.result.warn}</p>
          ) : null}
        </div>
      );
    }

    if (pane.kind === "admin") {
      return (
        <div className="min-h-0 flex-1 overflow-auto p-2">
          <HostDatabaseRelationalInspector
            instanceId={instanceId}
            engine={engine}
            schema={focusSchemaForInspector}
            canRunReadonlySql={canRunReadonlySql}
            dbCredsInvoke={dbCredsInvoke}
          />
        </div>
      );
    }

    return null;
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.key !== "Enter") return;
      if (!tab || tab.kind !== "sql") return;
      e.preventDefault();
      void runSqlTab(tab);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tab, runSqlTab]);

  if (splitEnabled && secondaryTab) {
    return (
      <div className="flex min-h-0 min-w-0 flex-1 divide-x divide-red-900/20">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">{renderPane(tab)}</div>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">{renderPane(secondaryTab)}</div>
      </div>
    );
  }

  return <div className="flex min-h-0 min-w-0 flex-1 flex-col">{renderPane(tab)}</div>;
}
