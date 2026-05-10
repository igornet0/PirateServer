import React, { useCallback, useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useI18n } from "../i18n";
import { hostDbAdminCreateSupported } from "./hostDbAdminSupport";

type QueryResult = {
  columns: string[];
  rows: Array<Record<string, unknown> | string | number | boolean | null>;
  row_count: number;
  truncated?: boolean;
  warn?: string;
};

type V2Caps = {
  workspace_v2?: boolean;
  admin_create_database?: boolean;
  admin_create_user?: boolean;
};

type Props = {
  instanceId: string;
  engine: string;
  /** Selected schema from parent (for table-privileges filter). */
  schema: string;
  canRunReadonlySql: boolean;
  dbCredsInvoke: () => Record<string, string | null | undefined>;
};

function rowKeysForGrid(rows: unknown[]): string[] {
  const keys = new Set<string>();
  for (const r of rows) {
    if (r && typeof r === "object" && !Array.isArray(r)) {
      for (const k of Object.keys(r as object)) keys.add(k);
    }
  }
  return Array.from(keys);
}

function gridFromObjects(rows: unknown[], cols: string[]) {
  if (rows.length === 0) {
    return <p className="p-2 text-xs text-slate-500">—</p>;
  }
  return (
    <div className="max-h-72 min-h-0 overflow-auto">
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
                const cell =
                  r && typeof r === "object" && c in (r as object) ? (r as Record<string, unknown>)[c] : null;
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
}

function sanitizePgIdent(s: string): string | null {
  const t = s.trim();
  if (!t || !/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(t)) return null;
  return t;
}

function sqlPostgresOverview(): string {
  return `SELECT current_database() AS database, session_user::text AS session_user, current_user::text AS current_role,
version() AS server_version,
(SELECT count(*)::bigint FROM pg_stat_activity WHERE datname = current_database()) AS connections_in_this_db`;
}

function sqlPostgresRoles(): string {
  return `SELECT rolname, rolsuper, rolinherit, rolcreaterole, rolcreatedb, rolcanlogin, rolconnlimit
FROM pg_roles ORDER BY rolname`;
}

function sqlPostgresDatabases(): string {
  return `SELECT d.datname AS database, pg_catalog.pg_get_userbyid(d.datdba)::text AS owner,
pg_catalog.pg_encoding_to_char(d.encoding) AS encoding,
pg_catalog.pg_database_size(d.oid)::bigint AS size_bytes
FROM pg_catalog.pg_database d WHERE NOT d.datistemplate ORDER BY d.datname`;
}

function sqlPostgresSchemaTableCounts(): string {
  return `SELECT table_schema, count(*)::bigint AS base_tables
FROM information_schema.tables
WHERE table_type = 'BASE TABLE'
  AND table_schema NOT IN ('pg_catalog', 'information_schema')
GROUP BY table_schema ORDER BY table_schema`;
}

function sqlPostgresTablePrivileges(schema: string): string {
  const sch = sanitizePgIdent(schema);
  const filter = sch
    ? `table_schema = '${sch.replace(/'/g, "''")}'`
    : `table_schema NOT IN ('pg_catalog', 'information_schema')`;
  return `SELECT table_schema, table_name, grantee, privilege_type, is_grantable
FROM information_schema.table_privileges
WHERE ${filter}
ORDER BY table_schema, table_name, grantee, privilege_type`;
}

function sqlPostgresDefaultPrivileges(): string {
  return `SELECT pg_get_userbyid(d.defaclrole)::text AS role,
n.nspname::text AS schema,
CASE d.defaclobjtype WHEN 'r' THEN 'table' WHEN 'S' THEN 'sequence' WHEN 'f' THEN 'function' WHEN 'T' THEN 'type' ELSE d.defaclobjtype::text END AS object_type,
pg_catalog.array_to_string(d.defaclacl, ', ') AS acl
FROM pg_default_acl d
JOIN pg_namespace n ON n.oid = d.defaclnamespace
ORDER BY 1, 2, 3`;
}

function sqlMysqlOverview(): string {
  return `SELECT DATABASE() AS database, USER() AS session_user, VERSION() AS server_version`;
}

function sqlMysqlGrantsCurrent(): string {
  return `SHOW GRANTS FOR CURRENT_USER()`;
}

function sqlMysqlSchemaTables(): string {
  return `SELECT table_schema, count(*) AS base_tables
FROM information_schema.tables
WHERE table_type = 'BASE TABLE' AND table_schema NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')
GROUP BY table_schema ORDER BY table_schema`;
}

function sqlMysqlAccounts(): string {
  return `SELECT User, Host FROM mysql.user ORDER BY User, Host`;
}

/** Preset read-only probes + optional admin create-user (control-api v2). */
export function HostDatabaseRelationalInspector({
  instanceId,
  engine,
  schema,
  canRunReadonlySql,
  dbCredsInvoke,
}: Props) {
  const { t, language } = useI18n();
  const tauri = isTauri();
  const [busy, setBusy] = useState(false);
  const [label, setLabel] = useState<string | null>(null);
  const [result, setResult] = useState<QueryResult | null>(null);
  const [localErr, setLocalErr] = useState<string | null>(null);
  const [inspectDb, setInspectDb] = useState(engine === "postgresql" ? "postgres" : "");

  const [caps, setCaps] = useState<V2Caps | null>(null);
  const [cuOpen, setCuOpen] = useState(false);
  const [cuDatabase, setCuDatabase] = useState("postgres");
  const [cuSchema, setCuSchema] = useState("public");
  const [cuShowSchema, setCuShowSchema] = useState(false);
  const [cuUsername, setCuUsername] = useState("");
  const [cuPrivileges, setCuPrivileges] = useState<"read_write" | "read_only">("read_write");
  const [cuGenPass, setCuGenPass] = useState(true);
  const [cuPassword, setCuPassword] = useState("");
  const [cuAllowDdl, setCuAllowDdl] = useState(false);
  const [cuResult, setCuResult] = useState<string | null>(null);

  const [duOpen, setDuOpen] = useState(false);
  const [duUsername, setDuUsername] = useState("");
  const [duDropOwned, setDuDropOwned] = useState(true);
  const [duResult, setDuResult] = useState<string | null>(null);

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

  useEffect(() => {
    setResult(null);
    setLabel(null);
    setLocalErr(null);
    setCuResult(null);
    setDuResult(null);
    setDuUsername("");
    setInspectDb(engine === "postgresql" ? "postgres" : "");
  }, [instanceId, engine]);

  const runSql = async (sql: string, titleKey: string) => {
    if (!canRunReadonlySql) {
      setLocalErr(t("db.inspectorNoSql"));
      return;
    }
    setLocalErr(null);
    setBusy(true);
    setLabel(t(titleKey));
    try {
      const dbArg =
        inspectDb.trim().length > 0 ? inspectDb.trim() : engine === "postgresql" ? "postgres" : null;
      const json = await invoke<string>("control_api_host_db_query_json", {
        instanceId,
        sql,
        maxRows: 500,
        database: dbArg,
        ...dbCredsInvoke(),
      });
      setResult(JSON.parse(json) as QueryResult);
    } catch (e) {
      setResult(null);
      setLocalErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const canAdminCreateUser = engine === "postgresql" && caps?.workspace_v2 && caps?.admin_create_user !== false;

  const createUserPayload = useMemo(() => {
    const database = (cuDatabase.trim() || "postgres") as string;
    const username = cuUsername.trim();
    const schema = (cuSchema.trim() || "public") as string;
    const o: Record<string, unknown> = {
      database,
      username,
      generate_password: cuGenPass,
      privileges: cuPrivileges,
      allow_schema_ddl: cuAllowDdl,
    };
    if (!cuGenPass && cuPassword.trim().length > 0) o.password = cuPassword;
    if (schema !== "public" || cuShowSchema) o.schema = schema;
    return o;
  }, [
    cuDatabase,
    cuUsername,
    cuSchema,
    cuShowSchema,
    cuGenPass,
    cuPassword,
    cuPrivileges,
    cuAllowDdl,
  ]);

  const createUserRequestJson = useMemo(() => JSON.stringify(createUserPayload, null, 2), [createUserPayload]);

  const submitCreateUser = async () => {
    if (!cuUsername.trim()) return;
    setCuResult(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_v2_admin_create_user_json", {
        instanceId,
        bodyJson: createUserRequestJson,
        ...dbCredsInvoke(),
      });
      setCuResult(j);
    } catch (e) {
      setCuResult(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const submitDeleteUser = async () => {
    const u = duUsername.trim();
    if (!u) return;
    const ok = window.confirm(
      language === "ru"
        ? `Удалить роль PostgreSQL «${u}»?${duDropOwned ? " Будет выполнено DROP OWNED во всех базах, затем DROP ROLE." : " Только DROP ROLE (может не удаться, если у роли остались объекты)." }`
        : `Drop PostgreSQL role «${u}»?${duDropOwned ? " This runs DROP OWNED in every database, then DROP ROLE." : " This runs DROP ROLE only (may fail if the role still owns objects)." }`,
    );
    if (!ok) return;
    setDuResult(null);
    setBusy(true);
    try {
      const body = { username: u, drop_owned_all_databases: duDropOwned };
      const j = await invoke<string>("control_api_host_db_v2_admin_delete_user_json", {
        instanceId,
        bodyJson: JSON.stringify(body),
        ...dbCredsInvoke(),
      });
      setDuResult(j);
    } catch (e) {
      setDuResult(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!tauri) return null;

  const showJumpServerAdmin =
    Boolean(
      caps?.workspace_v2 &&
        caps?.admin_create_database &&
        hostDbAdminCreateSupported(engine),
    );

  const btn =
    "rounded border border-border-subtle bg-black/25 px-2 py-1 text-[11px] text-slate-200 hover:bg-black/35 disabled:opacity-45";

  return (
    <div className="space-y-2 rounded border border-violet-900/35 bg-violet-950/15 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="text-xs font-semibold text-violet-100/90">{t("db.inspectorTitle")}</div>
        {showJumpServerAdmin ? (
          <button
            type="button"
            className="text-[10px] text-violet-200/90 underline decoration-dotted"
            onClick={() =>
              document
                .getElementById("pirate-host-db-server-toolbar")
                ?.scrollIntoView({ behavior: "smooth", block: "start" })
            }
          >
            {t("db.inspectorJumpServerAdmin")}
          </button>
        ) : null}
      </div>
      {!canRunReadonlySql ? (
        <p className="text-[10px] text-amber-200/80">{t("db.inspectorNoSql")}</p>
      ) : null}

      {(engine === "postgresql" || engine === "mysql") && canRunReadonlySql ? (
        <label className="flex flex-wrap items-center gap-2 text-[10px] text-slate-400">
          {t("db.inspectorTargetDb")}
          <input
            className="rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono text-slate-200"
            value={inspectDb}
            onChange={(e) => setInspectDb(e.target.value)}
            placeholder={engine === "postgresql" ? "postgres" : "mydb"}
          />
        </label>
      ) : null}

      <div className="flex flex-wrap gap-1.5">
        {engine === "postgresql" ? (
          <>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlPostgresOverview(), "db.inspectorOverview")}>
              {t("db.inspectorOverview")}
            </button>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlPostgresRoles(), "db.inspectorRoles")}>
              {t("db.inspectorRoles")}
            </button>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlPostgresDatabases(), "db.inspectorDatabases")}>
              {t("db.inspectorDatabases")}
            </button>
            <button
              type="button"
              disabled={busy}
              className={btn}
              onClick={() => void runSql(sqlPostgresSchemaTableCounts(), "db.inspectorSchemasTables")}
            >
              {t("db.inspectorSchemasTables")}
            </button>
            <button
              type="button"
              disabled={busy}
              className={btn}
              onClick={() => void runSql(sqlPostgresTablePrivileges(schema), "db.inspectorTablePrivileges")}
              title={schema ? `${t("db.inspectorTablePrivileges")}: ${schema}` : t("db.inspectorTablePrivilegesAll")}
            >
              {schema ? `${t("db.inspectorTablePrivileges")} (${schema})` : t("db.inspectorTablePrivileges")}
            </button>
            <button
              type="button"
              disabled={busy}
              className={btn}
              onClick={() => void runSql(sqlPostgresDefaultPrivileges(), "db.inspectorDefaultPrivileges")}
            >
              {t("db.inspectorDefaultPrivileges")}
            </button>
          </>
        ) : engine === "mysql" ? (
          <>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlMysqlOverview(), "db.inspectorOverview")}>
              {t("db.inspectorOverview")}
            </button>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlMysqlGrantsCurrent(), "db.inspectorGrantsCurrent")}>
              {t("db.inspectorGrantsCurrent")}
            </button>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlMysqlSchemaTables(), "db.inspectorSchemasTables")}>
              {t("db.inspectorSchemasTables")}
            </button>
            <button type="button" disabled={busy} className={btn} onClick={() => void runSql(sqlMysqlAccounts(), "db.inspectorMysqlAccounts")}>
              {t("db.inspectorMysqlAccounts")}
            </button>
          </>
        ) : null}
      </div>

      {engine === "postgresql" ? (
        <div className="space-y-2 border-t border-border-subtle/40 pt-2">
          <button
            type="button"
            className="text-[11px] text-violet-200/90 underline decoration-dotted"
            onClick={() => setCuOpen((o) => !o)}
          >
            {cuOpen ? "▼ " : "▶ "}
            {t("db.inspectorCreateUser")}
          </button>
          {cuOpen ? (
            <div className="space-y-2 rounded border border-border-subtle/50 bg-black/20 p-2 text-[10px]">
              {!canAdminCreateUser ? (
                <p className="text-amber-200/80">{t("db.inspectorCreateUserNeedsV2")}</p>
              ) : (
                <p className="text-slate-500">{t("db.inspectorCreateUserHint")}</p>
              )}
              <div className="grid max-w-2xl grid-cols-1 gap-2 sm:grid-cols-2">
                <label className="block text-slate-400">
                  {t("db.inspectorCuDatabase")}
                  <input
                    className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono text-slate-200"
                    value={cuDatabase}
                    onChange={(e) => setCuDatabase(e.target.value)}
                    placeholder="postgres"
                    spellCheck={false}
                  />
                </label>
                <label className="block text-slate-400">
                  {t("db.inspectorCuUsername")}
                  <input
                    className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono text-slate-200"
                    value={cuUsername}
                    onChange={(e) => setCuUsername(e.target.value)}
                    placeholder="app_user"
                    autoComplete="off"
                    spellCheck={false}
                  />
                </label>
                <label className="flex items-center gap-2 text-slate-400 sm:col-span-2">
                  <input type="checkbox" checked={cuGenPass} onChange={(e) => setCuGenPass(e.target.checked)} />
                  {t("db.inspectorCuGenPass")}
                </label>
                {!cuGenPass ? (
                  <label className="block text-slate-400 sm:col-span-2">
                    {t("db.inspectorCuPassword")}
                    <input
                      type="password"
                      className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono"
                      value={cuPassword}
                      onChange={(e) => setCuPassword(e.target.value)}
                      autoComplete="new-password"
                    />
                  </label>
                ) : null}
                <label className="block text-slate-400">
                  {t("db.inspectorCuPrivileges")}
                  <select
                    className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono text-slate-200"
                    value={cuPrivileges}
                    onChange={(e) => setCuPrivileges(e.target.value as "read_write" | "read_only")}
                  >
                    <option value="read_write">read_write</option>
                    <option value="read_only">read_only</option>
                  </select>
                </label>
                <label className="flex items-center gap-2 text-slate-400 sm:items-end sm:pb-0.5">
                  <input type="checkbox" checked={cuAllowDdl} onChange={(e) => setCuAllowDdl(e.target.checked)} />
                  {t("db.inspectorCuAllowDdl")}
                </label>
                <div className="sm:col-span-2">
                  <button
                    type="button"
                    className="text-slate-500 underline decoration-dotted"
                    onClick={() => setCuShowSchema((s) => !s)}
                  >
                    {cuShowSchema ? "▼ " : "▶ "}
                    {t("db.inspectorCuSchemaAdvanced")}
                  </button>
                  {cuShowSchema ? (
                    <label className="mt-1 block text-slate-400">
                      {t("db.inspectorCuSchema")}
                      <input
                        className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono text-slate-200"
                        value={cuSchema}
                        onChange={(e) => setCuSchema(e.target.value)}
                        placeholder="public"
                        spellCheck={false}
                      />
                    </label>
                  ) : null}
                </div>
              </div>
              <div className="space-y-0.5">
                <div className="text-[9px] font-medium text-slate-500">{t("db.inspectorCreateUserJsonPreview")}</div>
                <pre className="max-h-32 overflow-auto whitespace-pre-wrap rounded border border-border-subtle/50 bg-black/40 p-2 font-mono text-[9px] text-slate-400">
                  {createUserRequestJson}
                </pre>
              </div>
              <button
                type="button"
                disabled={busy || !canAdminCreateUser || !cuUsername.trim()}
                className="rounded border border-amber-800/50 bg-amber-950/35 px-2 py-1 text-[11px] text-amber-100"
                onClick={() => void submitCreateUser()}
              >
                {t("db.inspectorCuSubmit")}
              </button>
              {cuResult ? (
                <pre className="max-h-36 overflow-auto whitespace-pre-wrap rounded border border-border-subtle/50 bg-black/30 p-2 text-[10px] text-slate-300">
                  {cuResult}
                </pre>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      {engine === "postgresql" ? (
        <div className="space-y-2 border-t border-border-subtle/40 pt-2">
          <button
            type="button"
            className="text-[11px] text-rose-200/90 underline decoration-dotted"
            onClick={() => setDuOpen((o) => !o)}
          >
            {duOpen ? "▼ " : "▶ "}
            {t("db.inspectorDeleteUser")}
          </button>
          {duOpen ? (
            <div className="space-y-2 rounded border border-rose-900/30 bg-black/20 p-2 text-[10px]">
              {!canAdminCreateUser ? (
                <p className="text-amber-200/80">{t("db.inspectorCreateUserNeedsV2")}</p>
              ) : (
                <p className="text-slate-500">{t("db.inspectorDeleteUserHint")}</p>
              )}
              <label className="block text-slate-400">
                {t("db.inspectorDuUsername")}
                <input
                  className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 font-mono text-slate-200"
                  value={duUsername}
                  onChange={(e) => setDuUsername(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                />
              </label>
              <label className="flex items-center gap-2 text-slate-500">
                <input type="checkbox" checked={duDropOwned} onChange={(e) => setDuDropOwned(e.target.checked)} />
                {t("db.inspectorDuDropOwned")}
              </label>
              <pre className="max-h-24 overflow-auto rounded border border-border-subtle/50 bg-black/40 p-2 font-mono text-[9px] text-slate-400">
                {JSON.stringify(
                  { username: duUsername.trim() || "app_user", drop_owned_all_databases: duDropOwned },
                  null,
                  2,
                )}
              </pre>
              <button
                type="button"
                disabled={busy || !canAdminCreateUser || !duUsername.trim()}
                className="rounded border border-rose-800/50 bg-rose-950/35 px-2 py-1 text-[11px] text-rose-100"
                onClick={() => void submitDeleteUser()}
              >
                {t("db.inspectorDuSubmit")}
              </button>
              {duResult ? (
                <pre className="max-h-36 overflow-auto whitespace-pre-wrap rounded border border-border-subtle/50 bg-black/30 p-2 text-[10px] text-slate-300">
                  {duResult}
                </pre>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      {localErr ? <p className="text-[10px] text-rose-300">{localErr}</p> : null}
      {label ? <div className="text-[10px] font-medium text-slate-500">{label}</div> : null}
      {result &&
      (result.columns.length > 0 || (result.rows && result.rows.length > 0)) &&
      gridFromObjects(
        (result.rows ?? []) as unknown[],
        result.columns.length ? result.columns : rowKeysForGrid((result.rows ?? []) as unknown[]),
      )}
      {result?.truncated ? (
        <p className="text-[10px] text-amber-200/70">{language === "ru" ? "Усечено (лимит строк)" : "Truncated (row limit)"}</p>
      ) : null}
      {result?.warn ? <p className="text-[10px] text-amber-200/80">{result.warn}</p> : null}
    </div>
  );
}
