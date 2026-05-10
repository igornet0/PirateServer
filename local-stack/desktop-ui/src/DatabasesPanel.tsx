import { ChevronRight, Copy, KeyRound, RefreshCw } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { desktopDbAuthRequired } from "./desktopDbAuthConfig";
import { useI18n } from "./i18n";
import { DatabasesWorkspaceV2 } from "./databases/DatabasesWorkspaceV2";
import { HostDatabaseServerToolbar } from "./databases/HostDatabaseServerToolbar";
import { HostDbWorkspace } from "./databases/workspace/HostDbWorkspace";

type HostDatabaseCapabilities = {
  metadata: boolean;
  list_schemas: boolean;
  list_tables: boolean;
  list_columns: boolean;
  preview_rows: boolean;
  foreign_keys: boolean;
  run_readonly_sql: boolean;
  list_redis_keys: boolean;
  list_mongo_databases: boolean;
  list_mongo_collections: boolean;
  preview_mongo_documents: boolean;
  clickhouse_system: boolean;
};

export type HostDatabaseInstance = {
  id: string;
  engine: string;
  label: string;
  host: string;
  port: number;
  reachable: boolean;
  dsn_template: string;
  connection_note?: string | null;
  capabilities: HostDatabaseCapabilities;
};

type Props = {
  instances: HostDatabaseInstance[];
  onRefresh: () => Promise<void>;
  loadingList: boolean;
};

function localDsn(tpl: string, host: string, port: number, localPort: number): string {
  return tpl.replaceAll(`${host}:${port}`, `127.0.0.1:${localPort}`);
}

/** Masks `user:password@` in common DSN schemes for on-screen copy. */
function maskDsn(tpl: string): string {
  if (!/^[a-z+.-]+:\/\//i.test(tpl)) {
    return tpl;
  }
  return tpl.replace(/^([a-z+.-]+:\/\/)([^/@]+)@/i, (_m, scheme: string, userinfo: string) => {
    const u = String(userinfo).split(":")[0] ?? "user";
    return `${scheme}${u}:***@`;
  });
}

type QueryResult = {
  columns: string[];
  rows: Array<Record<string, unknown> | string | number | boolean | null>;
  row_count: number;
  truncated?: boolean;
  warn?: string;
};

function rowKeysForGrid(rows: unknown[]): string[] {
  const keys = new Set<string>();
  for (const r of rows) {
    if (r && typeof r === "object" && !Array.isArray(r)) {
      for (const k of Object.keys(r as object)) {
        keys.add(k);
      }
    }
  }
  return Array.from(keys);
}

export function DatabasesPanel({ instances, onRefresh, loadingList }: Props) {
  const { t, language } = useI18n();
  const tauri = isTauri();
  const [selId, setSelId] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [localPort, setLocalPort] = useState<number | null>(null);

  const [sql, setSql] = useState("SELECT 1");
  const [qResult, setQResult] = useState<QueryResult | null>(null);

  const [redisPattern, setRedisPattern] = useState("*");
  const [redisCursor, setRedisCursor] = useState("0");
  const [redisKeys, setRedisKeys] = useState<
    { key: string; type_name?: string | null; ttl_sec?: number | null }[]
  >([]);

  const [mongoDbs, setMongoDbs] = useState<string[]>([]);
  const [mongoDb, setMongoDb] = useState("");
  const [mongoColls, setMongoColls] = useState<string[]>([]);
  const [mongoColl, setMongoColl] = useState("");
  const [mongoDocs, setMongoDocs] = useState<unknown[]>([]);
  /** PG/MySQL: split heavy server admin from data workspace (mobile-friendly). */
  const [hostUiTab, setHostUiTab] = useState<"data" | "server" | "connection">("data");

  const [dbUser, setDbUser] = useState("");
  const [dbPassword, setDbPassword] = useState("");
  const [remember, setRemember] = useState(true);
  const [credsStatus, setCredsStatus] = useState<{
    username?: string;
    remember: boolean;
    hasSavedPassword: boolean;
  } | null>(null);

  const selected = useMemo(
    () => (selId ? instances.find((i) => i.id === selId) ?? null : null),
    [selId, instances],
  );

  const canBrowseContent = useMemo(() => {
    if (!desktopDbAuthRequired) return true;
    const u = dbUser.trim();
    if (!u) return false;
    if (dbPassword.trim().length > 0) return true;
    if (credsStatus?.hasSavedPassword) return true;
    return false;
  }, [dbUser, dbPassword, credsStatus]);

  /** PG/MySQL metadata + read-only SQL: allow when API advertises it, or when the user can pass per-request creds (old servers used list_schemas:false without env URLs). */
  const canUseRelationalHostDb = useMemo(() => {
    if (!selected) return false;
    if (selected.engine !== "postgresql" && selected.engine !== "mysql") return false;
    return selected.capabilities.list_schemas || canBrowseContent;
  }, [selected, canBrowseContent]);

  const dbCredsInvoke = useCallback(() => {
    if (!desktopDbAuthRequired) {
      return {} as Record<string, string | null | undefined>;
    }
    return {
      dbUser: dbUser.trim() || null,
      dbPassword: dbPassword.trim() ? dbPassword : null,
    };
  }, [dbUser, dbPassword]);

  useEffect(() => {
    if (instances.length > 0 && (!selId || !instances.some((i) => i.id === selId))) {
      setSelId(instances[0].id);
    }
  }, [instances, selId]);

  useEffect(() => {
    if (!tauri) return;
    void (async () => {
      const p = await invoke<number | null>("db_local_forward_local_port");
      setLocalPort(p ?? null);
    })();
  }, [tauri, selId]);

  useEffect(() => {
    if (!tauri || !selId || !desktopDbAuthRequired) {
      if (!desktopDbAuthRequired) setCredsStatus(null);
      return;
    }
    void (async () => {
      try {
        const j = await invoke<string>("db_credentials_get_json", { instanceId: selId });
        const s = JSON.parse(j) as {
          username?: string;
          remember?: boolean;
          hasSavedPassword?: boolean;
        };
        setCredsStatus({
          username: s.username,
          remember: s.remember !== false,
          hasSavedPassword: Boolean(s.hasSavedPassword),
        });
        if (s.username) setDbUser(s.username);
        setRemember(s.remember !== false);
      } catch {
        setCredsStatus({ remember: true, hasSavedPassword: false });
      }
    })();
  }, [tauri, selId, desktopDbAuthRequired]);

  useEffect(() => {
    setQResult(null);
    setRedisKeys([]);
    setRedisCursor("0");
    setMongoDbs([]);
    setMongoDb("");
    setMongoColls([]);
    setMongoColl("");
    setMongoDocs([]);
    setErr(null);
    if (selected?.engine === "clickhouse") {
      setSql("SELECT 1");
    } else {
      setSql("SELECT 1");
    }
    if (desktopDbAuthRequired) {
      setDbPassword("");
    }
    setHostUiTab("data");
  }, [selId, selected?.id, desktopDbAuthRequired]);

  const onTunnelStart = async () => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      const port = await invoke<number>("db_local_forward_start", {
        targetHost: selected.host,
        targetPort: selected.port,
      });
      setLocalPort(port);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onTunnelStop = async () => {
    setErr(null);
    setBusy(true);
    try {
      await invoke("db_local_forward_stop");
      setLocalPort(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const runQuery = async () => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      const json = await invoke<string>("control_api_host_db_query_json", {
        instanceId: selected.id,
        sql: sql,
        maxRows: 500,
        database: null,
        ...dbCredsInvoke(),
      });
      setQResult(JSON.parse(json) as QueryResult);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const loadRedis = async (cursor: string) => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_redis_keys_json", {
        instanceId: selected.id,
        pattern: redisPattern,
        cursor,
        ...dbCredsInvoke(),
      });
      const p = JSON.parse(j) as {
        keys: { key: string; type_name?: string; ttl_sec?: number }[];
        cursor: string;
        done: boolean;
      };
      setRedisKeys(p.keys);
      setRedisCursor(p.cursor);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const loadMongoDbs = async () => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_mongo_databases_json", {
        instanceId: selected.id,
        ...dbCredsInvoke(),
      });
      const dbs = JSON.parse(j) as string[];
      setMongoDbs(dbs);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const loadMongoColls = async (db: string) => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_mongo_collections_json", {
        instanceId: selected.id,
        db,
        ...dbCredsInvoke(),
      });
      setMongoColls(JSON.parse(j) as string[]);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const loadMongoPreview = async (db: string, coll: string) => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_mongo_preview_json", {
        instanceId: selected.id,
        db,
        collection: coll,
        limit: 50,
        ...dbCredsInvoke(),
      });
      setMongoDocs(JSON.parse(j) as unknown[]);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const copyDsn = (text: string) => {
    void navigator.clipboard.writeText(text);
  };

  const onSaveDbCreds = async () => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      await invoke("db_credentials_save", {
        instanceId: selected.id,
        username: dbUser.trim(),
        password: dbPassword,
        remember,
      });
      const j = await invoke<string>("db_credentials_get_json", { instanceId: selected.id });
      const s = JSON.parse(j) as { hasSavedPassword?: boolean; remember?: boolean };
      setCredsStatus((prev) => ({
        username: dbUser.trim() || undefined,
        remember: s.remember !== false,
        hasSavedPassword: Boolean(s.hasSavedPassword),
      }));
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onForgetDbCreds = async () => {
    if (!selected) return;
    setErr(null);
    setBusy(true);
    try {
      await invoke("db_credentials_forget", { instanceId: selected.id });
      setDbPassword("");
      setCredsStatus({ remember: false, hasSavedPassword: false });
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!tauri) {
    return (
      <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-100">
        {t("storage.tauriOnly")}
      </div>
    );
  }

  const dsnForCopy = selected
    ? localPort != null
      ? localDsn(selected.dsn_template, selected.host, selected.port, localPort)
      : selected.dsn_template
    : "";

  const dsnTemplateShown = selected
    ? localPort != null
      ? maskDsn(localDsn(selected.dsn_template, selected.host, selected.port, localPort))
      : maskDsn(selected.dsn_template)
    : "";

  const gridFromObjects = (rows: unknown[], cols: string[]) => {
    if (rows.length === 0) {
      return <p className="p-2 text-xs text-slate-500">—</p>;
    }
    return (
      <div className="max-h-80 min-h-0 overflow-auto">
        <table className="w-full min-w-[20rem] border-collapse text-left text-[11px]">
          <thead>
            <tr className="border-b border-border-subtle bg-black/30 text-slate-500">
              {cols.map((c) => (
                <th key={c} className="px-2 py-1 font-medium">
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={i} className="border-b border-border-subtle/40">
                {cols.map((c) => {
                  const cell = r && typeof r === "object" && c in (r as object) ? (r as Record<string, unknown>)[c] : null;
                  return (
                    <td key={c} className="max-w-xs truncate px-2 py-1 font-mono text-slate-200">
                      {cell === null || cell === undefined
                        ? "—"
                        : typeof cell === "object"
                          ? JSON.stringify(cell)
                          : String(cell)}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  };

  const hostConnectionStripEl = selected ? (
    <details className="rounded border border-border-subtle bg-black/20 [&_summary::-webkit-details-marker]:hidden">
      <summary className="cursor-pointer list-none px-3 py-2 text-[10px] font-medium text-slate-500 hover:bg-white/[0.03] hover:text-slate-400">
        {t("db.connectionStripSummary")}
      </summary>
      <div className="space-y-2 border-t border-border-subtle/50 px-3 pb-3 pt-2">
        <div className="text-[10px] text-slate-500">{t("db.dsn")}</div>
        <pre className="max-h-24 overflow-auto whitespace-pre-wrap break-all font-mono text-[10px] text-orange-200/90">
          {dsnTemplateShown}
        </pre>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() => copyDsn(dsnForCopy)}
            className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1 text-xs text-slate-100"
          >
            <Copy className="h-3.5 w-3.5" />
            {t("db.copyDsn")}
          </button>
          {localPort == null ? (
            <button
              type="button"
              disabled={busy}
              onClick={() => void onTunnelStart()}
              className="inline-flex items-center gap-1 rounded border border-amber-800/50 bg-amber-950/40 px-2 py-1 text-xs text-amber-100"
            >
              <KeyRound className="h-3.5 w-3.5" />
              {t("db.tunnelStart")}
            </button>
          ) : (
            <button
              type="button"
              disabled={busy}
              onClick={() => void onTunnelStop()}
              className="rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs text-slate-200"
            >
              {t("db.tunnelStop")}
            </button>
          )}
          {localPort != null ? (
            <span className="text-xs text-slate-400">
              {t("db.tunnelLocal")}: <code className="text-amber-200/80">{localPort}</code>
            </span>
          ) : null}
        </div>
        <p className="text-[10px] text-slate-500">{t("db.tunnelHint")}</p>
        {selected.connection_note ? (
          <p className="text-[10px] text-amber-200/60">
            {t("db.note")}: {selected.connection_note}
          </p>
        ) : null}
      </div>
    </details>
  ) : null;

  const hostCredentialsEl = desktopDbAuthRequired ? (
    <div className="space-y-2 rounded border border-border-subtle bg-black/25 p-3">
      <div className="text-[10px] font-medium text-slate-400">
        {language === "ru" ? "Доступ к данным БД" : "Database content access"}
      </div>
      <div className="flex flex-wrap gap-2">
        <label className="text-xs text-slate-400">
          {t("db.authUser")}
          <input
            className="ml-1 mt-0.5 block w-full min-w-0 max-w-full rounded border border-border-subtle bg-black/30 px-2 py-1 font-mono text-[11px] text-slate-200 sm:w-44"
            value={dbUser}
            onChange={(e) => setDbUser(e.target.value)}
            autoComplete="off"
          />
        </label>
        <label className="text-xs text-slate-400">
          {t("db.authPass")}
          <input
            type="password"
            className="ml-1 mt-0.5 block w-full min-w-0 max-w-full rounded border border-border-subtle bg-black/30 px-2 py-1 font-mono text-[11px] text-slate-200 sm:w-44"
            value={dbPassword}
            onChange={(e) => setDbPassword(e.target.value)}
            autoComplete="new-password"
          />
        </label>
      </div>
      <label className="flex items-center gap-2 text-[10px] text-slate-500">
        <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
        {t("db.authRemember")}
      </label>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className="rounded border border-amber-800/40 bg-amber-950/30 px-2 py-1 text-xs text-amber-100"
          disabled={busy}
          onClick={() => void onSaveDbCreds()}
        >
          {t("db.authSave")}
        </button>
        <button
          type="button"
          className="rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs text-slate-300"
          disabled={busy}
          onClick={() => void onForgetDbCreds()}
        >
          {t("db.authForget")}
        </button>
      </div>
      {credsStatus?.hasSavedPassword ? (
        <p className="text-[10px] text-slate-500">{t("db.authSavedPassHint")}</p>
      ) : (
        <p className="text-[10px] text-amber-200/60">{t("db.authNoSaved")}</p>
      )}
    </div>
  ) : null;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border-subtle bg-panel p-2 shadow-card">
        <h2 className="text-sm font-semibold text-slate-200">{t("db.title")}</h2>
        <button
          type="button"
          disabled={loadingList}
          onClick={() => void onRefresh()}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-black/20 px-2 py-1.5 text-xs text-slate-200 hover:bg-black/30 disabled:opacity-50"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          {t("db.refresh")}
        </button>
      </div>

      {err ? <p className="rounded border border-rose-900/50 bg-rose-950/30 px-3 py-2 text-xs text-rose-200">{err}</p> : null}

      {instances.length === 0 ? (
        <p className="text-xs text-slate-500">{t("db.empty")}</p>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-2 lg:min-h-[24rem] lg:flex-row">
          <div className="flex max-h-44 shrink-0 flex-row gap-1 overflow-x-auto overflow-y-hidden rounded border border-border-subtle p-1 lg:max-h-none lg:w-56 lg:flex-col lg:overflow-y-auto lg:overflow-x-hidden lg:p-0">
            {instances.map((i) => (
              <button
                key={i.id}
                type="button"
                onClick={() => setSelId(i.id)}
                className={`min-w-[9rem] shrink-0 rounded-md border border-transparent px-3 py-2 text-left text-xs touch-manipulation last:border-b-0 lg:min-w-0 lg:w-full lg:rounded-none lg:border-0 lg:border-b lg:border-border-subtle/40 lg:px-2 lg:py-2 ${
                  selId === i.id ? "bg-red-950/40 text-amber-100 lg:bg-red-950/30" : "text-slate-300 hover:bg-white/5"
                }`}
              >
                <div className="font-medium">{i.label}</div>
                <div className="mt-0.5 font-mono text-[10px] text-slate-500">
                  {i.engine} · {i.host}:{i.port}
                </div>
                {!i.reachable ? (
                  <div className="text-[10px] text-rose-400/80">{t("db.unreachable")}</div>
                ) : null}
              </button>
            ))}
          </div>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
            {selected ? (
              <>
                {canUseRelationalHostDb ? (
                  <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2">
                    <div
                      className="sticky top-0 z-[1] flex gap-1 overflow-x-auto rounded-lg border border-border-subtle bg-panel/95 p-1 shadow-sm backdrop-blur-sm [-webkit-overflow-scrolling:touch]"
                      role="tablist"
                      aria-label={t("db.title")}
                    >
                      {(
                        [
                          ["data", t("db.hostSectionData")] as const,
                          ["server", t("db.hostSectionServer")] as const,
                          ["connection", t("db.hostSectionConnection")] as const,
                        ] as const
                      ).map(([id, label]) => (
                        <button
                          key={id}
                          type="button"
                          role="tab"
                          aria-selected={hostUiTab === id}
                          onClick={() => setHostUiTab(id)}
                          className={`touch-manipulation whitespace-nowrap rounded-md px-3 py-2.5 text-xs font-medium sm:py-2 ${
                            hostUiTab === id ? "bg-red-950/50 text-amber-100" : "text-slate-400 hover:bg-white/5"
                          }`}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                    {hostUiTab === "server" ? (
                      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto">
                        <HostDatabaseServerToolbar
                          instanceId={selected.id}
                          engine={selected.engine}
                          canBrowse={canBrowseContent}
                          dbCredsInvoke={dbCredsInvoke}
                        />
                        <DatabasesWorkspaceV2
                          instanceId={selected.id}
                          canBrowse={canBrowseContent}
                          dbCredsInvoke={dbCredsInvoke}
                        />
                      </div>
                    ) : null}
                    {hostUiTab === "connection" ? (
                      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto">
                        {hostConnectionStripEl}
                        {hostCredentialsEl}
                      </div>
                    ) : null}
                    {hostUiTab === "data" ? (
                      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
                        {desktopDbAuthRequired && !canBrowseContent ? (
                          <p className="rounded border border-amber-900/40 bg-amber-950/20 px-3 py-2 text-xs text-amber-100/90">
                            {t("db.authBlocked")}
                          </p>
                        ) : null}
                        {canBrowseContent ? (
                          <HostDbWorkspace
                            instanceId={selected.id}
                            engine={selected.engine}
                            canBrowse={canBrowseContent}
                            canRunReadonlySql={selected.capabilities.run_readonly_sql}
                            dbCredsInvoke={dbCredsInvoke}
                          />
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                ) : (
                  <>
                    <HostDatabaseServerToolbar
                      instanceId={selected.id}
                      engine={selected.engine}
                      canBrowse={canBrowseContent}
                      dbCredsInvoke={dbCredsInvoke}
                    />
                    <DatabasesWorkspaceV2
                      instanceId={selected.id}
                      canBrowse={canBrowseContent}
                      dbCredsInvoke={dbCredsInvoke}
                    />
                    {hostConnectionStripEl}
                    {hostCredentialsEl}
                  </>
                )}

                {!canUseRelationalHostDb && desktopDbAuthRequired && !canBrowseContent ? (
                  <p className="rounded border border-amber-900/40 bg-amber-950/20 px-3 py-2 text-xs text-amber-100/90">
                    {t("db.authBlocked")}
                  </p>
                ) : null}

                {selected.engine === "oracle" ? (
                  <p className="text-xs text-slate-400">
                    {language === "ru"
                      ? "Просмотр в приложении не подключён — используйте DSN и туннель с DBeaver."
                      : "In-app browser is not wired for Oracle — use DSN and tunnel with DBeaver."}
                  </p>
                ) : null}

                {canBrowseContent ? (
                <>
                {selected.engine === "clickhouse" && selected.capabilities.run_readonly_sql ? (
                  <div className="space-y-2">
                    <div className="text-[10px] text-slate-500">
                      {language === "ru" ? "Метаданные через system.* — SQL ниже" : "Use system.* via SQL below"}
                    </div>
                    <textarea
                      className="h-24 w-full rounded border border-border-subtle bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                      value={sql}
                      onChange={(e) => setSql(e.target.value)}
                    />
                    <button
                      type="button"
                      className="rounded border border-amber-800/40 bg-amber-950/30 px-3 py-1.5 text-xs text-amber-100"
                      onClick={() => void runQuery()}
                      disabled={busy}
                    >
                      {t("db.run")}
                    </button>
                    {qResult &&
                    (qResult.columns.length > 0 || (qResult.row_count > 0 && qResult.rows?.length)) ? (
                      gridFromObjects(
                        (qResult.rows ?? []) as unknown[],
                        qResult.columns.length
                          ? qResult.columns
                          : rowKeysForGrid((qResult.rows ?? []) as unknown[]),
                      )
                    ) : null}
                    {qResult && qResult.warn ? (
                      <p className="text-[10px] text-amber-200/80">{qResult.warn}</p>
                    ) : null}
                  </div>
                ) : null}

                {selected.engine === "redis" && selected.capabilities.list_redis_keys ? (
                  <div className="space-y-2">
                    <div className="flex flex-wrap gap-2">
                      <input
                        className="rounded border border-border-subtle bg-black/30 px-2 py-1 font-mono text-xs"
                        value={redisPattern}
                        onChange={(e) => setRedisPattern(e.target.value)}
                        placeholder={t("db.redisPattern")}
                      />
                      <button
                        type="button"
                        className="rounded border border-border-subtle bg-black/20 px-2 py-1 text-xs"
                        onClick={() => void loadRedis("0")}
                        disabled={busy}
                      >
                        {t("db.run")}
                      </button>
                      <button
                        type="button"
                        className="rounded border border-border-subtle bg-black/20 px-2 py-1 text-xs"
                        onClick={() => void loadRedis(redisCursor)}
                        disabled={busy || !redisKeys.length}
                      >
                        {t("db.redisMore")}
                        <ChevronRight className="inline h-3 w-3" />
                      </button>
                    </div>
                    <ul className="max-h-48 overflow-auto text-[11px] text-slate-300">
                      {redisKeys.map((k) => (
                        <li key={k.key} className="border-b border-border-subtle/30 py-0.5 font-mono">
                          {k.key}
                          {k.type_name ? <span className="ml-2 text-slate-500">({k.type_name})</span> : null}
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}

                {selected.engine === "mongodb" && selected.capabilities.list_mongo_databases ? (
                  <div className="space-y-2">
                    <div className="flex flex-wrap gap-2">
                      <select
                        className="rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs"
                        value={mongoDb}
                        onChange={(e) => {
                          const d = e.target.value;
                          setMongoDb(d);
                          setMongoColl("");
                          if (d) void loadMongoColls(d);
                        }}
                      >
                        <option value="">{t("db.mongoDb")}</option>
                        {mongoDbs.map((d) => (
                          <option key={d} value={d}>
                            {d}
                          </option>
                        ))}
                      </select>
                      <button
                        type="button"
                        className="rounded border border-border-subtle bg-black/20 px-2 py-1 text-xs"
                        onClick={() => void loadMongoDbs()}
                        disabled={busy}
                      >
                        {t("db.loadMeta")}
                      </button>
                    </div>
                    {mongoDb ? (
                      <div className="flex flex-wrap gap-2">
                        <select
                          className="rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs"
                          value={mongoColl}
                          onChange={(e) => {
                            const c = e.target.value;
                            setMongoColl(c);
                            if (c) void loadMongoPreview(mongoDb, c);
                          }}
                        >
                          <option value="">{t("db.mongoColl")}</option>
                          {mongoColls.map((c) => (
                            <option key={c} value={c}>
                              {c}
                            </option>
                          ))}
                        </select>
                      </div>
                    ) : null}
                    {mongoDocs.length > 0 ? (
                      <pre className="max-h-64 overflow-auto rounded border border-border-subtle p-2 text-[10px] text-slate-400">
                        {t("db.mongoPreview")}
                        {"\n"}
                        {JSON.stringify(mongoDocs, null, 2)}
                      </pre>
                    ) : null}
                  </div>
                ) : null}
                </>
                ) : null}
              </>
            ) : (
              <p className="text-xs text-slate-500">
                {language === "ru" ? "Выберите экземпляр слева." : "Select an instance on the left."}
              </p>
            )}
          </div>
        </div>
      )}

      {busy ? <p className="text-[10px] text-slate-500">…</p> : null}
    </div>
  );
}
