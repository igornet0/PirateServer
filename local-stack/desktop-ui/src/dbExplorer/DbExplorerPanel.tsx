import Editor from "@monaco-editor/react";
import { Database, Download, Link2, Loader2, Pencil, Play, Shield, Trash2, Unplug } from "lucide-react";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { useI18n } from "../i18n";
import { DbDataGrid } from "../databases/workspace/DbDataGrid";
import { DbDrawer } from "../databases/workspace/DbDrawer";
import { DbTabsBar } from "../databases/workspace/DbTabsBar";
import {
  useDbExplorerStore,
  type CenterTab,
  type DirectProfile,
  type QueryResult,
} from "./dbExplorerStore";
import { PgStatsCharts } from "./PgStatsCharts";

function parseJson<T>(s: string): T {
  return JSON.parse(s) as T;
}

function toCsv(r: QueryResult): string {
  const esc = (x: unknown) => {
    const t = x === null || x === undefined ? "" : String(x);
    if (/[",\n]/.test(t)) return `"${t.replace(/"/g, '""')}"`;
    return t;
  };
  const head = r.columns.map(esc).join(",");
  const lines = r.rows.map((row) => r.columns.map((c) => esc((row as Record<string, unknown>)[c])).join(","));
  return [head, ...lines].join("\n");
}

function downloadText(filename: string, text: string, mime: string) {
  const blob = new Blob([text], { type: mime });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = filename;
  a.click();
  URL.revokeObjectURL(a.href);
}

type DbExplorerPanelProps = {
  /** Renders as a card inside Storage → Databases (no top-level nav). Softer chrome. */
  embedInStorage?: boolean;
};

export function DbExplorerPanel({ embedInStorage = false }: DbExplorerPanelProps) {
  const { t, language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const tauri = isTauri();
  const st = useDbExplorerStore();
  const {
    profiles,
    sessionId,
    sessionProfileId,
    activeProfileId,
    connectBusy,
    lastError,
    schemas,
    selectedSchema,
    tables,
    selectedTable,
    preview,
    sqlText,
    sqlResult,
    sqlBusy,
    centerTab,
    historyJson,
    statsJson,
    structureJson,
    selectProfile,
  } = st;

  const [oneShotPassword, setOneShotPassword] = useState("");
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<Partial<DirectProfile> | null>(null);
  const [tunnels, setTunnels] = useState<string | null>(null);
  const [tcpId, setTcpId] = useState("f1");
  const [rowDetail, setRowDetail] = useState<{ title: string; body: string } | null>(null);
  const [profileMenu, setProfileMenu] = useState<{ x: number; y: number; profileId: string } | null>(null);
  const profileMenuRef = useRef<HTMLDivElement | null>(null);

  const [tcpHost, setTcpHost] = useState("127.0.0.1");
  const [tcpPort, setTcpPort] = useState(5432);
  const [sshId, setSshId] = useState("s1");
  const [sshHost, setSshHost] = useState("bastion");
  const [sshPort, setSshPort] = useState(22);
  const [sshUser, setSshUser] = useState("ubuntu");
  const [remoteHost, setRemoteHost] = useState("10.0.0.1");
  const [remotePort, setRemotePort] = useState(5432);
  const [localPortSsh, setLocalPortSsh] = useState(0);

  const loadProfiles = useCallback(async () => {
    if (!tauri) return;
    const raw = await invoke<string>("db_direct_profile_list_json");
    const j = parseJson<DirectProfile[]>(raw);
    st.setProfiles(j);
  }, [tauri, st]);

  useEffect(() => {
    void loadProfiles();
  }, [loadProfiles]);

  useEffect(() => {
    if (profiles.length > 0 && !activeProfileId) {
      selectProfile(profiles[0]!.id);
    }
    if (profiles.length === 0 && activeProfileId) {
      selectProfile(null);
    }
  }, [profiles, activeProfileId, selectProfile]);

  useEffect(() => {
    if (!profileMenu) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setProfileMenu(null);
    const onDoc = (e: PointerEvent) => {
      const root = profileMenuRef.current;
      if (root && e.target instanceof Node && root.contains(e.target)) return;
      setProfileMenu(null);
    };
    const t = window.setTimeout(() => {
      document.addEventListener("pointerdown", onDoc);
    }, 0);
    window.addEventListener("keydown", onKey);
    return () => {
      window.clearTimeout(t);
      document.removeEventListener("pointerdown", onDoc);
      window.removeEventListener("keydown", onKey);
    };
  }, [profileMenu]);

  const refreshTunnels = useCallback(async () => {
    if (!tauri) return;
    setTunnels(await invoke<string>("db_tunnel_list_json"));
  }, [tauri]);

  useEffect(() => {
    if (centerTab === "tunnels" && tauri) void refreshTunnels();
  }, [centerTab, tauri, refreshTunnels]);

  const connect = useCallback(
    async (profileId: string) => {
      if (!tauri) return;
      st.setConnectBusy(true);
      st.setLastError(null);
      try {
        const v = await invoke<unknown>("db_direct_open", {
          req: { profileId, password: oneShotPassword || null },
        });
        const sid = (v as { sessionId?: string }).sessionId;
        if (!sid) throw new Error("no sessionId");
        st.setSession(sid, profileId);
        setOneShotPassword("");
        const sraw = await invoke<string>("db_direct_list_schemas", { sessionId: sid });
        const sch = parseJson<string[]>(sraw);
        st.setSchemas(sch);
        st.setSelectedSchema(sch[0] ?? null);
        st.setLastError(null);
        toast.success(tr("Подключено", "Connected"));
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        st.setLastError(msg);
        toast.error(msg);
      } finally {
        st.setConnectBusy(false);
      }
    },
    [tauri, st, tr, oneShotPassword],
  );

  const loadTables = useCallback(
    async (schema: string) => {
      if (!sessionId || !tauri) return;
      const raw = await invoke<string>("db_direct_list_tables", { sessionId, schema });
      st.setTables(parseJson<string[]>(raw));
    },
    [sessionId, tauri, st],
  );

  useEffect(() => {
    if (sessionId && selectedSchema) void loadTables(selectedSchema);
  }, [sessionId, selectedSchema, loadTables]);

  const loadPreview = useCallback(async () => {
    if (!sessionId || !selectedSchema || !selectedTable || !tauri) return;
    const raw = await invoke<string>("db_direct_table_preview", {
      req: { sessionId, schema: selectedSchema, table: selectedTable, limit: 200, offset: 0 },
    });
    st.setPreview(parseJson<QueryResult>(raw));
  }, [sessionId, selectedSchema, selectedTable, tauri, st]);

  useEffect(() => {
    if (centerTab === "data" && sessionId && selectedSchema && selectedTable) void loadPreview();
  }, [centerTab, sessionId, selectedSchema, selectedTable, loadPreview]);

  const runSql = useCallback(async () => {
    if (!sessionId || !tauri) return;
    st.setSqlBusy(true);
    try {
      const raw = await invoke<string>("db_direct_query", {
        req: { sessionId, sql: sqlText, maxRows: 2000 },
      });
      st.setSqlResult(parseJson<QueryResult>(raw));
      st.setHistoryJson(await invoke<string>("db_direct_query_history_list", { connectionId: sessionId, limit: 30 }));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      st.setSqlBusy(false);
    }
  }, [sessionId, tauri, sqlText, st]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && centerTab === "sql" && sessionId) {
        e.preventDefault();
        void runSql();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [centerTab, sessionId, runSql]);

  const loadStats = useCallback(async () => {
    if (!sessionId || !tauri) return;
    try {
      const j = await invoke<string>("db_direct_pg_stats", { sessionId });
      st.setStatsJson(j);
    } catch (e) {
      st.setStatsJson(
        JSON.stringify({ error: e instanceof Error ? e.message : String(e) }),
      );
    }
  }, [sessionId, tauri, st]);

  useEffect(() => {
    if (centerTab === "stats" && sessionId) void loadStats();
  }, [centerTab, sessionId, loadStats]);

  const loadStructure = useCallback(async () => {
    if (!sessionId || !selectedSchema || !selectedTable || !tauri) return;
    const j = await invoke<string>("db_direct_pg_structure", {
      req: { sessionId, schema: selectedSchema, table: selectedTable },
    });
    st.setStructureJson(j);
  }, [sessionId, selectedSchema, selectedTable, tauri, st]);

  useEffect(() => {
    if (centerTab === "structure" && sessionId && selectedSchema && selectedTable) void loadStructure();
  }, [centerTab, sessionId, selectedSchema, selectedTable, loadStructure]);

  const saveProfile = async () => {
    if (!editing) return;
    const body: Record<string, unknown> = {
      name: editing.name ?? "conn",
      engine: editing.engine ?? "postgres",
      host: editing.host ?? "127.0.0.1",
      port: typeof editing.port === "number" ? editing.port : 5432,
      databaseName: editing.databaseName ?? null,
      username: editing.username ?? null,
      sslMode: editing.sslMode ?? "prefer",
      groupTag: editing.groupTag ?? null,
      orderIndex: editing.orderIndex ?? 0,
    };
    if (editing.id) body.id = editing.id;
    await invoke("db_direct_profile_upsert", { body: JSON.stringify(body), password: oneShotPassword || null });
    setFormOpen(false);
    setEditing(null);
    setOneShotPassword("");
    void loadProfiles();
  };

  const openProfileEditor = (p: DirectProfile) => {
    setProfileMenu(null);
    setOneShotPassword("");
    setEditing({
      id: p.id,
      name: p.name,
      engine: p.engine,
      host: p.host,
      port: p.port,
      databaseName: p.databaseName,
      username: p.username,
      sslMode: p.sslMode,
      groupTag: p.groupTag,
      orderIndex: p.orderIndex,
    });
    setFormOpen(true);
  };

  const deleteDirectProfile = async (profileId: string) => {
    const p = profiles.find((x) => x.id === profileId);
    const label = p?.name ?? profileId;
    if (!window.confirm(tr(`Удалить профиль «${label}»?`, `Delete profile “${label}”?`))) return;
    setProfileMenu(null);
    const prevActive = activeProfileId;
    try {
      if (sessionId && sessionProfileId === profileId) {
        await invoke("db_direct_close", { sessionId });
        st.resetAfterDisconnect();
      }
      await invoke("db_direct_profile_delete", { id: profileId });
      const raw = await invoke<string>("db_direct_profile_list_json");
      const j = parseJson<DirectProfile[]>(raw);
      st.setProfiles(j);
      const still = j.some((x) => x.id === prevActive);
      if (prevActive === profileId || !still) {
        st.selectProfile(j[0]?.id ?? null);
      }
      toast.success(tr("Профиль удалён", "Profile removed"));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  if (!tauri) {
    return (
      <p className="p-4 text-sm text-amber-200/80">
        {t("storage.tauriOnly")}
      </p>
    );
  }

  const shell = embedInStorage
    ? "rounded-2xl border border-amber-500/20 bg-gradient-to-b from-slate-900/80 via-slate-950/90 to-black/50 shadow-[inset_0_1px_0_rgba(251,191,36,0.08),0_8px_32px_rgba(0,0,0,0.35)]"
    : "border-b border-red-900/20 bg-app";

  return (
    <div
      className={`flex min-h-0 flex-col text-[11px] text-slate-200 ${
        embedInStorage
          ? "h-[min(78vh,720px)] min-h-[min(78vh,720px)]"
          : "h-full"
      } ${shell}`}
    >
      <div
        className={`shrink-0 px-3 py-2 ${embedInStorage ? "rounded-t-2xl border-b border-amber-900/20 bg-amber-950/10" : "border-b border-red-900/30"}`}
      >
        <h2 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
          <Database className={`h-4 w-4 ${embedInStorage ? "text-amber-400/90" : "text-red-400"}`} />
          {tr("Прямое подключение (Tauri)", "Direct connection (Tauri)")}
        </h2>
        <p className="mt-1 text-[10px] leading-relaxed text-slate-500">
          {embedInStorage
            ? tr(
                "Профили в SQLite; пароль direct-профиля — в db_direct_passwords.json (AES, тот же ключ что host_db_credentials.key). БД на хосте Pirate: вкладка «Хост» (host_db_credentials.json).",
                "Profiles in SQLite; direct profile passwords in db_direct_passwords.json (AES-256-GCM, same key as host_db_credentials.key). Pirate host DB: “Host” tab (host_db_credentials.json).",
              )
            : tr(
                "Direct: пароль в JSON + шифрование; хост Pirate — «Хранилище» → «Базы» → «Хост».",
                "Direct: passwords in local JSON (encrypted). Managed host DB: Storage → Databases → Host.",
              )}
        </p>
      </div>
      <Group orientation="horizontal" className="min-h-0 flex-1">
        <Panel
          defaultSize={25}
          minSize={12}
          className={`flex min-h-0 min-w-0 flex-col p-2 ${embedInStorage ? "border-r border-amber-900/15 bg-black/15" : "border-r border-red-900/30 bg-black/20"}`}
          id="db-left"
        >
          <div className="mb-2 flex shrink-0 items-center justify-between">
            <span className="text-[10px] font-medium uppercase text-slate-500">{tr("Профили", "Profiles")}</span>
            <button
              type="button"
              onClick={() => {
                setEditing({ name: "postgres", engine: "postgres", host: "127.0.0.1", port: 5432, sslMode: "prefer" });
                setFormOpen(true);
              }}
              className="rounded border border-red-800/50 px-1.5 py-0.5 text-[10px] text-slate-200 hover:bg-red-950/30"
            >
              +
            </button>
          </div>
          <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto">
            {profiles.map((p) => (
              <li key={p.id}>
                <button
                  type="button"
                  onClick={() => selectProfile(p.id)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setProfileMenu({ x: e.clientX, y: e.clientY, profileId: p.id });
                  }}
                  className={`flex w-full items-center justify-between rounded px-2 py-1 text-left ${
                    activeProfileId === p.id ? "bg-red-950/50 text-amber-100" : "hover:bg-red-950/20"
                  }`}
                >
                  <span className="truncate font-mono text-[10px]">{p.name}</span>
                  <span className="shrink-0 text-[9px] text-slate-500">{p.engine}</span>
                </button>
              </li>
            ))}
          </ul>
          {profileMenu
            ? (() => {
                const p = profiles.find((x) => x.id === profileMenu.profileId);
                if (!p) return null;
                return (
                  <div
                    ref={profileMenuRef}
                    role="menu"
                    className="fixed z-[200] min-w-[180px] overflow-hidden rounded-md border border-slate-700/80 bg-slate-950/98 py-0.5 text-xs shadow-2xl ring-1 ring-slate-800"
                    style={{ left: profileMenu.x, top: profileMenu.y }}
                  >
                    <button
                      type="button"
                      role="menuitem"
                      className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-slate-200 hover:bg-slate-800/80"
                      onClick={() => openProfileEditor(p)}
                    >
                      <Pencil className="h-3.5 w-3.5 text-slate-400" />
                      {tr("Изменить", "Edit")}
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-rose-200 hover:bg-rose-950/50"
                      onClick={() => void deleteDirectProfile(p.id)}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      {tr("Удалить", "Delete")}
                    </button>
                  </div>
                );
              })()
            : null}
          {activeProfileId ? (
            <div className="mt-2 space-y-1 border-t border-white/5 pt-2">
              <input
                type="password"
                autoComplete="off"
                value={oneShotPassword}
                onChange={(e) => setOneShotPassword(e.target.value)}
                placeholder={tr("Пароль (если не сохранён в JSON)", "Password (if not saved in JSON)")}
                className="w-full rounded border border-red-900/30 bg-black/30 px-2 py-1 text-[10px] text-slate-100"
              />
              <button
                type="button"
                disabled={connectBusy}
                onClick={() => void connect(activeProfileId!)}
                className="inline-flex w-full items-center justify-center gap-1 rounded border border-amber-800/50 bg-amber-950/30 py-1 text-[10px] text-amber-100 disabled:opacity-50"
              >
                {connectBusy ? <Loader2 className="h-3 w-3 animate-spin" /> : <Link2 className="h-3 w-3" />}
                {tr("Подключить", "Connect")}
              </button>
            </div>
          ) : null}
          {lastError ? <p className="mt-1 text-[10px] text-rose-300">{lastError}</p> : null}
          {sessionId ? (
            <button
              type="button"
              onClick={async () => {
                await invoke("db_direct_close", { sessionId });
                st.resetAfterDisconnect();
                setOneShotPassword("");
              }}
              className="mt-2 inline-flex w-full items-center justify-center gap-1 rounded border border-rose-900/40 py-1 text-[10px] text-rose-200"
            >
              <Unplug className="h-3 w-3" />
              {tr("Отключить", "Disconnect")}
            </button>
          ) : null}
        </Panel>
        <Separator
          className={embedInStorage ? "w-1 bg-amber-950/25 hover:bg-amber-800/40" : "w-1 bg-red-950/30 hover:bg-red-800/50"}
          id="db-sep"
        />
        <Panel
          defaultSize={75}
          minSize={20}
          className={`relative min-w-0 flex min-h-0 flex-col p-0 ${embedInStorage ? "rounded-br-2xl" : ""}`}
          id="db-main"
        >
          <DbTabsBar
            tabs={[
              { id: "data", title: tr("Данные", "Data"), kind: "table_data" },
              { id: "sql", title: "SQL", kind: "sql" },
              { id: "stats", title: tr("Статистика", "Stats"), kind: "admin" },
              { id: "structure", title: tr("Структура", "Structure"), kind: "table_schema" },
              { id: "tunnels", title: "SSH/TCP", kind: "admin" },
            ]}
            activeId={centerTab}
            onActivate={(id) => st.setCenterTab(id as CenterTab)}
          />
          <div className="min-h-0 flex-1 overflow-hidden p-2">
            {centerTab === "data" && sessionId ? (
              <div className="flex h-full min-h-0 flex-col gap-2">
                <div className="flex flex-wrap gap-2">
                  <select
                    value={selectedSchema ?? ""}
                    onChange={(e) => {
                      st.setSelectedSchema(e.target.value || null);
                      st.setSelectedTable(null);
                    }}
                    className="rounded border border-red-900/30 bg-black/30 px-2 py-0.5 text-[10px]"
                  >
                    <option value="">{tr("схема", "schema")}</option>
                    {schemas.map((s) => (
                      <option key={s} value={s}>
                        {s}
                      </option>
                    ))}
                  </select>
                  <select
                    value={selectedTable ?? ""}
                    onChange={(e) => st.setSelectedTable(e.target.value || null)}
                    className="rounded border border-red-900/30 bg-black/30 px-2 py-0.5 text-[10px]"
                  >
                    <option value="">{tr("таблица", "table")}</option>
                    {tables.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </select>
                </div>
                {preview ? (
                  <div className="relative min-h-0 flex-1">
                    <DbDataGrid
                      columns={preview.columns.map((c) => ({ name: c }))}
                      rows={preview.rows as unknown[]}
                      onRowDetail={(i) => {
                        const r = preview.rows[i];
                        setRowDetail({
                          title: `${selectedSchema ?? "?"}.${selectedTable ?? "?"} #${i + 1}`,
                          body: typeof r === "object" ? JSON.stringify(r, null, 2) : String(r),
                        });
                      }}
                    />
                  </div>
                ) : (
                  <p className="text-slate-500">{tr("Выберите таблицу", "Choose a table")}</p>
                )}
                {preview ? (
                  <div className="flex gap-1">
                    <button
                      type="button"
                      onClick={() => downloadText("result.csv", toCsv(preview), "text/csv")}
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-1.5 py-0.5 text-[10px] text-slate-300"
                    >
                      <Download className="h-3 w-3" />
                      CSV
                    </button>
                    <button
                      type="button"
                      onClick={() =>
                        downloadText("result.json", JSON.stringify(preview.rows, null, 2), "application/json")
                      }
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-1.5 py-0.5 text-[10px] text-slate-300"
                    >
                      JSON
                    </button>
                  </div>
                ) : null}
              </div>
            ) : null}
            {centerTab === "data" && !sessionId ? <p className="text-slate-500">{tr("Сначала подключитесь", "Connect first")}</p> : null}

            {centerTab === "sql" && sessionId ? (
              <div className="flex h-full min-h-0 flex-col gap-2">
                <div className="min-h-0 flex-1 overflow-hidden rounded border border-red-900/30">
                  <Editor
                    height="12rem"
                    defaultLanguage="sql"
                    theme="vs-dark"
                    value={sqlText}
                    onChange={(v) => st.setSqlText(v ?? "")}
                    options={{ minimap: { enabled: false }, fontSize: 12, wordWrap: "on" }}
                  />
                </div>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() => void runSql()}
                    disabled={sqlBusy}
                    className="inline-flex items-center gap-1 rounded border border-amber-800/50 bg-amber-950/30 px-2 py-1 text-[10px] text-amber-100"
                  >
                    {sqlBusy ? <Loader2 className="h-3 w-3 animate-spin" /> : <Play className="h-3 w-3" />}
                    {tr("Выполнить (Ctrl+Enter)", "Run (Ctrl+Enter)")}
                  </button>
                </div>
                {sqlResult ? (
                  <div className="relative min-h-0 flex-1">
                    <DbDataGrid
                      columns={sqlResult.columns.map((c) => ({ name: c }))}
                      rows={sqlResult.rows as unknown[]}
                      onRowDetail={(i) => {
                        const r = sqlResult.rows[i];
                        setRowDetail({
                          title: `SQL #${i + 1}`,
                          body: typeof r === "object" ? JSON.stringify(r, null, 2) : String(r),
                        });
                      }}
                    />
                  </div>
                ) : null}
                {sqlResult ? (
                  <div className="flex gap-1">
                    <button
                      type="button"
                      onClick={() => downloadText("query.csv", toCsv(sqlResult), "text/csv")}
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-1.5 py-0.5 text-[10px] text-slate-300"
                    >
                      <Download className="h-3 w-3" />
                      CSV
                    </button>
                    <button
                      type="button"
                      onClick={() =>
                        downloadText("query.json", JSON.stringify(sqlResult.rows, null, 2), "application/json")
                      }
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-1.5 py-0.5 text-[10px] text-slate-300"
                    >
                      JSON
                    </button>
                  </div>
                ) : null}
                {historyJson ? (
                  <pre className="mt-1 max-h-24 overflow-auto rounded border border-white/5 bg-black/20 p-1 text-[9px] text-slate-500">
                    {historyJson}
                  </pre>
                ) : null}
              </div>
            ) : null}
            {centerTab === "sql" && !sessionId ? <p className="text-slate-500">{tr("Сначала подключитесь", "Connect first")}</p> : null}

            {centerTab === "stats" && sessionId && statsJson ? <PgStatsCharts raw={statsJson} /> : null}
            {centerTab === "stats" && sessionId && !statsJson ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {centerTab === "stats" && !sessionId ? <p className="text-slate-500">{tr("Сначала подключитесь (PG)", "Connect (PostgreSQL)")}</p> : null}

            {centerTab === "structure" && sessionId && structureJson ? (
              <pre className="max-h-full overflow-auto text-[10px] text-slate-300">{structureJson}</pre>
            ) : null}
            {centerTab === "structure" && !sessionId ? <p className="text-slate-500">{tr("Сначала подключитесь", "Connect first")}</p> : null}

            {centerTab === "tunnels" ? (
              <div className="space-y-2 text-[10px] text-slate-300">
                <p className="text-slate-500">
                  {tr(
                    "Статусы: TCP — в процессе; SSH — sidecar `ssh` (нужен OpenSSH в PATH).",
                    "TCP runs in-app; SSH uses OpenSSH in PATH (sidecar).",
                  )}
                </p>
                <pre className="max-h-24 overflow-auto rounded border border-white/5 bg-black/20 p-1">{tunnels ?? "[]"}</pre>
                <div className="flex flex-wrap items-end gap-1">
                  <input value={tcpId} onChange={(e) => setTcpId(e.target.value)} className="w-20 rounded border border-white/10 bg-black/30 px-1" />
                  <input value={tcpHost} onChange={(e) => setTcpHost(e.target.value)} className="w-24 rounded border border-white/10 bg-black/30 px-1" />
                  <input
                    type="number"
                    value={tcpPort}
                    onChange={(e) => setTcpPort(parseInt(e.target.value, 10) || 0)}
                    className="w-16 rounded border border-white/10 bg-black/30 px-1"
                  />
                  <button
                    type="button"
                    onClick={async () => {
                      const p = await invoke<number>("db_tunnel_tcp_start", { id: tcpId, targetHost: tcpHost, targetPort: tcpPort });
                      toast.message(`TCP → ${p}`);
                      void refreshTunnels();
                    }}
                    className="rounded border border-amber-800/50 px-1.5 py-0.5"
                  >
                    TCP
                  </button>
                </div>
                <div className="grid grid-cols-2 gap-1">
                  <label className="text-[9px] text-slate-500">
                    id
                    <input value={sshId} onChange={(e) => setSshId(e.target.value)} className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1" />
                  </label>
                  <label className="text-[9px] text-slate-500">
                    ssh host
                    <input value={sshHost} onChange={(e) => setSshHost(e.target.value)} className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1" />
                  </label>
                  <label className="text-[9px] text-slate-500">
                    ssh port
                    <input
                      type="number"
                      value={sshPort}
                      onChange={(e) => setSshPort(parseInt(e.target.value, 10) || 0)}
                      className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1"
                    />
                  </label>
                  <label className="text-[9px] text-slate-500">
                    user
                    <input value={sshUser} onChange={(e) => setSshUser(e.target.value)} className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1" />
                  </label>
                  <label className="text-[9px] text-slate-500">
                    remote host
                    <input value={remoteHost} onChange={(e) => setRemoteHost(e.target.value)} className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1" />
                  </label>
                  <label className="text-[9px] text-slate-500">
                    remote port
                    <input
                      type="number"
                      value={remotePort}
                      onChange={(e) => setRemotePort(parseInt(e.target.value, 10) || 0)}
                      className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1"
                    />
                  </label>
                  <label className="text-[9px] text-slate-500">
                    local (0=auto)
                    <input
                      type="number"
                      value={localPortSsh}
                      onChange={(e) => setLocalPortSsh(parseInt(e.target.value, 10) || 0)}
                      className="mt-0.5 w-full rounded border border-white/10 bg-black/30 px-1"
                    />
                  </label>
                </div>
                <button
                  type="button"
                  onClick={async () => {
                    const p = await invoke<number>("db_tunnel_ssh_start", {
                      id: sshId,
                      sshHost: sshHost,
                      sshPort: sshPort,
                      sshUser: sshUser,
                      remoteHost: remoteHost,
                      remotePort: remotePort,
                      localPort: localPortSsh,
                      identityPath: null,
                    });
                    toast.message(`SSH L2R local :${p}`);
                    void refreshTunnels();
                  }}
                  className="inline-flex items-center gap-1 rounded border border-slate-600 px-2 py-1"
                >
                  <Shield className="h-3 w-3" />
                  SSH
                </button>
              </div>
            ) : null}
          </div>
          <DbDrawer
            open={rowDetail != null}
            title={rowDetail?.title ?? ""}
            onClose={() => setRowDetail(null)}
            widthClassName="max-w-xl"
          >
            <pre className="whitespace-pre-wrap break-all font-mono text-[10px] text-slate-300">{rowDetail?.body}</pre>
          </DbDrawer>
        </Panel>
      </Group>

      {formOpen && editing ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-sm rounded-lg border border-red-900/50 bg-slate-900 p-3 shadow-2xl">
            <h3 className="text-xs font-semibold text-slate-100">{tr("Профиль", "Profile")}</h3>
            {(["name", "host", "port", "username", "engine", "sslMode", "databaseName"] as const).map((f) => (
              <label key={f} className="mt-1 block text-[10px] text-slate-400">
                {f}
                <input
                  value={String((editing as never)[f] ?? "")}
                  onChange={(e) => {
                    const v = e.target.value;
                    setEditing((x) => ({ ...(x ?? {}), [f]: f === "port" ? parseInt(v, 10) : v } as DirectProfile));
                  }}
                  className="mt-0.5 w-full rounded border border-white/10 bg-black/40 px-1 text-slate-200"
                />
              </label>
            ))}
            <input
              type="password"
              value={oneShotPassword}
              onChange={(e) => setOneShotPassword(e.target.value)}
              placeholder={tr("пароль (сохранить в JSON)", "password (saved to encrypted JSON)")}
              className="mt-2 w-full rounded border border-red-800/30 bg-black/30 px-1 text-[10px]"
            />
            <div className="mt-2 flex justify-end gap-1">
              <button type="button" onClick={() => setFormOpen(false)} className="rounded px-2 py-0.5 text-[10px] text-slate-400">
                {tr("Отмена", "Cancel")}
              </button>
              <button
                type="button"
                onClick={() => void saveProfile()}
                className="rounded border border-amber-700/50 bg-amber-950/30 px-2 py-0.5 text-[10px] text-amber-100"
              >
                {tr("Сохранить", "Save")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
