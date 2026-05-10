import React, { useCallback, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useI18n } from "../i18n";

type V2Caps = {
  workspace_v2?: boolean;
  write?: boolean;
  sql_jobs?: boolean;
  migration_status?: boolean;
  admin_create_database?: boolean;
  admin_create_table?: boolean;
  admin_create_user?: boolean;
  migration_run?: boolean;
};

type ObjectTree = {
  version?: number;
  engine?: string;
  schemas?: Array<{ name: string; tables: string[] }>;
};

type Props = {
  instanceId: string | null;
  canBrowse: boolean;
  dbCredsInvoke: () => Record<string, string | null | undefined>;
};

/** DBeaver-like v2: metadata tree, grid preview, async SQL job. Server admin / migration tools: see HostDatabaseServerToolbar. */
export function DatabasesWorkspaceV2({ instanceId, canBrowse, dbCredsInvoke }: Props) {
  const { t } = useI18n();
  const tauri = isTauri();
  const [caps, setCaps] = useState<V2Caps | null>(null);
  const [tree, setTree] = useState<ObjectTree | null>(null);
  const [treeErr, setTreeErr] = useState<string | null>(null);
  const [gridJson, setGridJson] = useState<string | null>(null);
  const [jobStart, setJobStart] = useState<string | null>(null);
  const [jobPoll, setJobPoll] = useState<string | null>(null);
  const [sqlJobSql, setSqlJobSql] = useState("SELECT 1");
  const [busy, setBusy] = useState(false);

  const loadCaps = useCallback(async () => {
    if (!tauri) return;
    try {
      const j = await invoke<string>("control_api_host_db_v2_capabilities_json");
      setCaps(JSON.parse(j) as V2Caps);
    } catch {
      setCaps({ workspace_v2: false });
    }
  }, [tauri]);

  useEffect(() => {
    void loadCaps();
  }, [loadCaps]);

  if (!tauri || !instanceId) return null;
  if (!caps?.workspace_v2) return null;

  return (
    <div className="space-y-2 rounded border border-cyan-900/40 bg-cyan-950/20 p-3">
      <div className="text-xs font-semibold text-cyan-100/90">{t("db.workspaceV2")}</div>
      <p className="text-[10px] text-slate-500">
        {t("db.workspaceV2BrowseHint")}{" "}
        <span className="font-mono text-slate-400">
          write={String(caps.write)} sql_jobs={String(caps.sql_jobs)}
        </span>
      </p>

      {canBrowse ? (
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={async () => {
              setTreeErr(null);
              setBusy(true);
              try {
                const j = await invoke<string>("control_api_host_db_v2_object_tree_json", {
                  instanceId,
                  ...dbCredsInvoke(),
                });
                setTree(JSON.parse(j) as ObjectTree);
              } catch (e) {
                setTreeErr(e instanceof Error ? e.message : String(e));
                setTree(null);
              } finally {
                setBusy(false);
              }
            }}
            className="rounded border border-cyan-800/50 bg-cyan-950/40 px-2 py-1 text-[11px] text-cyan-100"
          >
            {t("db.v2LoadTree")}
          </button>
          <button
            type="button"
            disabled={busy || !tree?.schemas?.[0]}
            onClick={async () => {
              const sch = tree?.schemas?.[0]?.name;
              const tbl = tree?.schemas?.[0]?.tables?.[0];
              if (!sch || !tbl) return;
              setBusy(true);
              setGridJson(null);
              try {
                const j = await invoke<string>("control_api_host_db_v2_grid_json", {
                  instanceId,
                  schema: sch,
                  table: tbl,
                  limit: 50,
                  offset: 0,
                  sortColumn: null,
                  sortDesc: false,
                  filterColumn: null,
                  filterValue: null,
                  ...dbCredsInvoke(),
                });
                setGridJson(j);
              } catch (e) {
                setGridJson(e instanceof Error ? e.message : String(e));
              } finally {
                setBusy(false);
              }
            }}
            className="rounded border border-cyan-800/50 bg-cyan-950/40 px-2 py-1 text-[11px] text-cyan-100"
          >
            {t("db.v2LoadFirstGrid")}
          </button>
        </div>
      ) : (
        <p className="text-[10px] text-slate-500">{t("db.authBlocked")}</p>
      )}

      {treeErr ? <p className="text-[10px] text-rose-300">{treeErr}</p> : null}
      {tree ? (
        <pre className="max-h-40 overflow-auto rounded border border-border-subtle bg-black/30 p-2 font-mono text-[10px] text-slate-300">
          {JSON.stringify(tree, null, 2)}
        </pre>
      ) : null}
      {gridJson ? (
        <pre className="max-h-40 overflow-auto rounded border border-border-subtle bg-black/30 p-2 font-mono text-[10px] text-slate-300">
          {gridJson.length > 4000 ? `${gridJson.slice(0, 4000)}…` : gridJson}
        </pre>
      ) : null}

      {caps.sql_jobs && canBrowse ? (
        <div className="space-y-1 border-t border-cyan-900/30 pt-2">
          <div className="text-[10px] text-slate-500">{t("db.v2SqlJob")}</div>
          <textarea
            className="min-h-[3rem] w-full rounded border border-border-subtle bg-black/20 p-1 font-mono text-[10px] text-slate-200"
            value={sqlJobSql}
            onChange={(e) => setSqlJobSql(e.target.value)}
          />
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                setJobStart(null);
                setJobPoll(null);
                try {
                  const j = await invoke<string>("control_api_host_db_v2_sql_job_start_json", {
                    instanceId,
                    sql: sqlJobSql,
                    maxRows: 200,
                    ...dbCredsInvoke(),
                  });
                  setJobStart(j);
                  const p = JSON.parse(j) as { job_id?: string; status?: string };
                  if (p.job_id) {
                    const poll = await invoke<string>("control_api_host_db_v2_sql_job_get_json", {
                      instanceId,
                      jobId: p.job_id,
                      ...dbCredsInvoke(),
                    });
                    setJobPoll(poll);
                  }
                } catch (e) {
                  setJobStart(e instanceof Error ? e.message : String(e));
                } finally {
                  setBusy(false);
                }
              }}
              className="rounded border border-cyan-800/50 bg-cyan-950/40 px-2 py-1 text-[11px] text-cyan-100"
            >
              {t("db.v2SqlJobRun")}
            </button>
          </div>
          {jobStart ? (
            <pre className="max-h-24 overflow-auto text-[10px] text-slate-400">{jobStart}</pre>
          ) : null}
          {jobPoll ? (
            <pre className="max-h-32 overflow-auto text-[10px] text-slate-400">{jobPoll}</pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
