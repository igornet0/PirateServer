import React, { useCallback, useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useI18n } from "../i18n";
import { hostDbAdminCreateSupported } from "./hostDbAdminSupport";

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

type MigrationToolRow = {
  tool?: string;
  present?: boolean;
  current_version?: string | null;
  summary?: string | null;
  error?: string | null;
};

type MigrationStatusPayload = {
  engine?: string;
  database?: string;
  detected_at_ms?: number;
  tools?: MigrationToolRow[];
};

type SimpleCol = {
  id: string;
  name: string;
  dataType: string;
  varcharLen: string;
  not_null: boolean;
  primary_key: boolean;
};

type Props = {
  instanceId: string;
  engine: string;
  canBrowse: boolean;
  dbCredsInvoke: () => Record<string, string | null | undefined>;
};

function parseMigrationStatus(json: string): MigrationStatusPayload | null {
  try {
    return JSON.parse(json) as MigrationStatusPayload;
  } catch {
    return null;
  }
}

let simpleColSeq = 0;
function nextColId(): string {
  simpleColSeq += 1;
  return `c-${simpleColSeq}`;
}

const SIMPLE_DATA_TYPES = [
  "text",
  "varchar",
  "integer",
  "bigint",
  "boolean",
  "timestamptz",
  "jsonb",
  "uuid",
  "serial",
  "bigserial",
] as const;

/** Server-side DB actions: create/ensure DB, create table, migration probe, optional CLI run — host DB v2 control-api. */
export function HostDatabaseServerToolbar({ instanceId, engine, canBrowse, dbCredsInvoke }: Props) {
  const { t, language } = useI18n();
  const tauri = isTauri();
  const [caps, setCaps] = useState<V2Caps | null>(null);
  const [busy, setBusy] = useState(false);
  const [localErr, setLocalErr] = useState<string | null>(null);

  const [adminDb, setAdminDb] = useState("");
  const [adminOwner, setAdminOwner] = useState("");
  const [adminEnc, setAdminEnc] = useState("");
  const [adminResult, setAdminResult] = useState<string | null>(null);

  const [migrationDb, setMigrationDb] = useState(engine === "postgresql" ? "postgres" : "");
  const [migrationToolsFilter, setMigrationToolsFilter] = useState<"all" | "core">("core");
  const [migrationParsed, setMigrationParsed] = useState<MigrationStatusPayload | null>(null);
  const [migrationRaw, setMigrationRaw] = useState<string | null>(null);

  const [runTool, setRunTool] = useState("alembic");
  const [runWorkdir, setRunWorkdir] = useState("");
  const [runResult, setRunResult] = useState<string | null>(null);

  const [createTableMode, setCreateTableMode] = useState<"simple" | "json">("simple");
  const [ctDb, setCtDb] = useState(engine === "postgresql" ? "postgres" : "");
  const [ctSchema, setCtSchema] = useState("public");
  const [ctTable, setCtTable] = useState("");
  const [ctIfNotExists, setCtIfNotExists] = useState(true);
  const [simpleCols, setSimpleCols] = useState<SimpleCol[]>(() => [
    {
      id: nextColId(),
      name: "id",
      dataType: "bigserial",
      varcharLen: "",
      not_null: false,
      primary_key: true,
    },
    {
      id: nextColId(),
      name: "title",
      dataType: "text",
      varcharLen: "",
      not_null: true,
      primary_key: false,
    },
  ]);

  const [createTableJson, setCreateTableJson] = useState(
    '{\n  "database": "postgres",\n  "schema": "public",\n  "table": "example_items",\n  "if_not_exists": true,\n  "columns": [\n    { "name": "id", "data_type": "bigserial", "primary_key": true },\n    { "name": "title", "data_type": "text", "not_null": true }\n  ]\n}\n',
  );
  const [cuDatabase, setCuDatabase] = useState("postgres");
  const [cuSchema, setCuSchema] = useState("public");
  const [cuShowSchema, setCuShowSchema] = useState(false);
  const [cuUsername, setCuUsername] = useState("app_user");
  const [cuGenPass, setCuGenPass] = useState(true);
  const [cuPassword, setCuPassword] = useState("");
  const [cuPrivileges, setCuPrivileges] = useState<"read_write" | "read_only">("read_write");
  const [cuAllowDdl, setCuAllowDdl] = useState(false);
  const [adminTableResult, setAdminTableResult] = useState<string | null>(null);
  const [adminUserResult, setAdminUserResult] = useState<string | null>(null);

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
    setLocalErr(null);
    setAdminResult(null);
    setMigrationParsed(null);
    setMigrationRaw(null);
    setRunResult(null);
    setAdminTableResult(null);
    setAdminUserResult(null);
    setMigrationDb(engine === "postgresql" ? "postgres" : "");
    setCtDb(engine === "postgresql" ? "postgres" : "");
    setCtSchema("public");
    setCtTable("");
    setCreateTableMode("simple");
    simpleColSeq = 0;
    setSimpleCols([
      {
        id: nextColId(),
        name: "id",
        dataType: "bigserial",
        varcharLen: "",
        not_null: false,
        primary_key: true,
      },
      {
        id: nextColId(),
        name: "title",
        dataType: "text",
        varcharLen: "",
        not_null: true,
        primary_key: false,
      },
    ]);
    setCuDatabase("postgres");
    setCuSchema("public");
    setCuShowSchema(false);
    setCuUsername("app_user");
    setCuGenPass(true);
    setCuPassword("");
    setCuPrivileges("read_write");
    setCuAllowDdl(false);
  }, [instanceId, engine]);

  const createUserRequestJson = useMemo(() => {
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
    return JSON.stringify(o, null, 2);
  }, [cuDatabase, cuUsername, cuSchema, cuShowSchema, cuGenPass, cuPassword, cuPrivileges, cuAllowDdl]);

  if (!tauri) return null;
  if (!caps?.workspace_v2) {
    return (
      <div className="rounded border border-slate-700/50 bg-black/20 px-3 py-2 text-[10px] text-slate-400">
        {t("db.serverV2DisabledHint")}
      </div>
    );
  }

  const adminEngineOk = hostDbAdminCreateSupported(engine);
  const showAdminDb = Boolean(caps.admin_create_database && adminEngineOk);
  const showAdminCapsButUnsupported = Boolean(caps.admin_create_database && !adminEngineOk);
  const showCreateTableSection = Boolean(
    caps.admin_create_table !== false && caps.admin_create_database && adminEngineOk,
  );
  const showMigrationProbe =
    Boolean(caps.migration_status) &&
    canBrowse &&
    (engine === "postgresql" || engine === "mysql");
  const showMigrationRun = Boolean(caps.migration_run);
  const showAdvancedUserOnly =
    Boolean(caps.admin_create_database) &&
    engine === "postgresql" &&
    caps.admin_create_user !== false;

  const toolsQueryParam = migrationToolsFilter === "core" ? "alembic,prisma,flyway" : undefined;

  const loadMigrationStatus = async () => {
    setLocalErr(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_v2_migration_status_get_json", {
        instanceId,
        database: migrationDb.trim(),
        ...dbCredsInvoke(),
        tools: toolsQueryParam ?? null,
      });
      setMigrationRaw(j);
      setMigrationParsed(parseMigrationStatus(j));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setLocalErr(msg);
      setMigrationRaw(null);
      setMigrationParsed(null);
    } finally {
      setBusy(false);
    }
  };

  const runAdminCreate = async (ifNotExists: boolean) => {
    if (!adminDb.trim()) return;
    setLocalErr(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_v2_admin_create_database_json", {
        instanceId,
        database: adminDb.trim(),
        owner: engine === "postgresql" && adminOwner.trim() ? adminOwner.trim() : null,
        encoding: engine === "postgresql" && adminEnc.trim() ? adminEnc.trim() : null,
        if_not_exists: ifNotExists,
        ...dbCredsInvoke(),
      });
      setAdminResult(j);
    } catch (e) {
      setAdminResult(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const submitCreateTableJson = async (jsonStr: string) => {
    setAdminTableResult(null);
    setBusy(true);
    try {
      const j = await invoke<string>("control_api_host_db_v2_admin_create_table_json", {
        instanceId,
        bodyJson: jsonStr.trim(),
      });
      setAdminTableResult(j);
    } catch (e) {
      setAdminTableResult(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const buildSimpleTableJson = (): string | null => {
    if (!ctDb.trim() || !ctTable.trim()) return null;
    const schemaVal =
      engine === "mysql" ? ctDb.trim() : ctSchema.trim() || "public";
    const columns: Record<string, unknown>[] = [];
    for (const c of simpleCols) {
      if (!c.name.trim()) return null;
      const col: Record<string, unknown> = {
        name: c.name.trim(),
        data_type: c.dataType,
        not_null: c.not_null,
        primary_key: c.primary_key,
      };
      if (c.dataType === "varchar") {
        const n = parseInt(c.varcharLen, 10);
        if (!Number.isFinite(n) || n < 1) return null;
        col.varchar_length = n;
      }
      columns.push(col);
    }
    return JSON.stringify({
      database: ctDb.trim(),
      schema: schemaVal,
      table: ctTable.trim(),
      if_not_exists: ctIfNotExists,
      columns,
    });
  };

  const runSimpleCreateTable = async () => {
    const j = buildSimpleTableJson();
    if (!j) {
      setAdminTableResult(
        language === "ru"
          ? "Заполните имя БД, таблицы и для varchar — длину."
          : "Fill database, table, and varchar length where needed.",
      );
      return;
    }
    await submitCreateTableJson(j);
  };

  return (
    <div
      id="pirate-host-db-server-toolbar"
      className="space-y-3 rounded border border-amber-900/35 bg-amber-950/15 p-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="text-xs font-semibold text-amber-100/95">{t("db.serverActions")}</div>
        <span className="font-mono text-[9px] text-slate-500">
          v2 migration={String(caps.migration_status)} admin={String(caps.admin_create_database)} run=
          {String(caps.migration_run)}
        </span>
      </div>

      {localErr ? (
        <p className="rounded border border-rose-900/40 bg-rose-950/25 px-2 py-1 text-[10px] text-rose-200">{localErr}</p>
      ) : null}

      {showAdminCapsButUnsupported ? (
        <p className="rounded border border-amber-900/30 bg-amber-950/20 px-2 py-1 text-[10px] text-amber-200/90">
          {t("db.adminDdlNotSupportedEngine")}
        </p>
      ) : null}

      {showAdminDb ? (
        <div className="space-y-2 border-b border-border-subtle/40 pb-3">
          <div className="text-[10px] font-medium text-slate-400">{t("db.v2AdminCreate")}</div>
          <div className="flex flex-wrap items-end gap-2 text-[10px]">
            <label className="text-slate-400">
              {t("db.serverDbName")}
              <input
                className="ml-1 block rounded border border-border-subtle bg-black/25 px-1.5 py-0.5 font-mono text-slate-200"
                value={adminDb}
                onChange={(e) => setAdminDb(e.target.value)}
                placeholder="appdb"
              />
            </label>
            {engine === "postgresql" ? (
              <>
                <label className="text-slate-400">
                  {t("db.serverOwner")}
                  <input
                    className="ml-1 block rounded border border-border-subtle bg-black/25 px-1.5 py-0.5 font-mono text-slate-200"
                    value={adminOwner}
                    onChange={(e) => setAdminOwner(e.target.value)}
                  />
                </label>
                <label className="text-slate-400">
                  {t("db.serverEncoding")}
                  <input
                    className="ml-1 w-20 rounded border border-border-subtle bg-black/25 px-1.5 py-0.5 font-mono text-slate-200"
                    value={adminEnc}
                    onChange={(e) => setAdminEnc(e.target.value)}
                    placeholder="UTF8"
                  />
                </label>
              </>
            ) : null}
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={busy || !adminDb.trim()}
              onClick={() => void runAdminCreate(false)}
              className="rounded border border-amber-700/50 bg-amber-950/40 px-2 py-1 text-[11px] text-amber-100"
            >
              {t("db.serverCreateDbStrict")}
            </button>
            <button
              type="button"
              disabled={busy || !adminDb.trim()}
              onClick={() => void runAdminCreate(true)}
              className="rounded border border-slate-600/50 bg-black/30 px-2 py-1 text-[11px] text-slate-200"
            >
              {t("db.serverEnsureDb")}
            </button>
          </div>
          <p className="text-[9px] text-slate-500">
            {engine === "mysql" ? t("db.serverAdminMysqlHint") : t("db.serverAdminHint")}
          </p>
          {adminResult ? (
            <pre className="max-h-20 overflow-auto rounded border border-border-subtle/50 bg-black/20 p-1.5 text-[10px] text-slate-400">
              {adminResult}
            </pre>
          ) : null}
        </div>
      ) : null}

      {showCreateTableSection ? (
        <div className="space-y-2 border-b border-border-subtle/40 pb-3">
          <div className="text-[10px] font-medium text-slate-400">{t("db.createTableSectionTitle")}</div>
          <div className="flex flex-wrap gap-2 text-[10px]">
            <button
              type="button"
              className={`rounded px-2 py-0.5 ${createTableMode === "simple" ? "bg-amber-950/50 text-amber-100" : "bg-black/25 text-slate-400"}`}
              onClick={() => setCreateTableMode("simple")}
            >
              {t("db.createTableModeSimple")}
            </button>
            <button
              type="button"
              className={`rounded px-2 py-0.5 ${createTableMode === "json" ? "bg-amber-950/50 text-amber-100" : "bg-black/25 text-slate-400"}`}
              onClick={() => setCreateTableMode("json")}
            >
              {t("db.createTableModeJson")}
            </button>
          </div>

          {createTableMode === "simple" ? (
            <div className="space-y-2 text-[10px]">
              <div className="flex flex-wrap gap-2">
                <label className="text-slate-400">
                  {t("db.serverDbName")}
                  <input
                    className="ml-1 block w-28 rounded border border-border-subtle bg-black/25 px-1 font-mono text-slate-200"
                    value={ctDb}
                    onChange={(e) => setCtDb(e.target.value)}
                  />
                </label>
                {engine === "postgresql" ? (
                  <label className="text-slate-400">
                    {t("db.inspectorCuSchema")}
                    <input
                      className="ml-1 block w-24 rounded border border-border-subtle bg-black/25 px-1 font-mono text-slate-200"
                      value={ctSchema}
                      onChange={(e) => setCtSchema(e.target.value)}
                    />
                  </label>
                ) : null}
                <label className="text-slate-400">
                  {t("db.createTableTableName")}
                  <input
                    className="ml-1 block w-32 rounded border border-border-subtle bg-black/25 px-1 font-mono text-slate-200"
                    value={ctTable}
                    onChange={(e) => setCtTable(e.target.value)}
                  />
                </label>
                <label className="flex items-center gap-1 text-slate-500">
                  <input
                    type="checkbox"
                    checked={ctIfNotExists}
                    onChange={(e) => setCtIfNotExists(e.target.checked)}
                  />
                  IF NOT EXISTS
                </label>
              </div>
              <div className="space-y-1 rounded border border-border-subtle/40 bg-black/15 p-2">
                <div className="text-slate-500">{t("db.createTableColumns")}</div>
                {simpleCols.map((c) => (
                  <div key={c.id} className="flex flex-wrap items-center gap-1 border-b border-border-subtle/20 py-1">
                    <input
                      className="w-24 rounded border border-border-subtle bg-black/30 px-1 font-mono text-slate-200"
                      value={c.name}
                      placeholder="name"
                      onChange={(e) =>
                        setSimpleCols((prev) =>
                          prev.map((x) => (x.id === c.id ? { ...x, name: e.target.value } : x)),
                        )
                      }
                    />
                    <select
                      className="rounded border border-border-subtle bg-black/30 px-1 font-mono text-[10px] text-slate-200"
                      value={c.dataType}
                      onChange={(e) =>
                        setSimpleCols((prev) =>
                          prev.map((x) => (x.id === c.id ? { ...x, dataType: e.target.value } : x)),
                        )
                      }
                    >
                      {SIMPLE_DATA_TYPES.map((dt) => (
                        <option key={dt} value={dt}>
                          {dt}
                        </option>
                      ))}
                    </select>
                    {c.dataType === "varchar" ? (
                      <input
                        className="w-12 rounded border border-border-subtle bg-black/30 px-1 font-mono"
                        value={c.varcharLen}
                        placeholder="255"
                        onChange={(e) =>
                          setSimpleCols((prev) =>
                            prev.map((x) => (x.id === c.id ? { ...x, varcharLen: e.target.value } : x)),
                          )
                        }
                      />
                    ) : null}
                    <label className="flex items-center gap-0.5 text-slate-500">
                      <input
                        type="checkbox"
                        checked={c.not_null}
                        onChange={(e) =>
                          setSimpleCols((prev) =>
                            prev.map((x) => (x.id === c.id ? { ...x, not_null: e.target.checked } : x)),
                          )
                        }
                      />
                      NN
                    </label>
                    <label className="flex items-center gap-0.5 text-slate-500">
                      <input
                        type="checkbox"
                        checked={c.primary_key}
                        onChange={(e) =>
                          setSimpleCols((prev) =>
                            prev.map((x) => (x.id === c.id ? { ...x, primary_key: e.target.checked } : x)),
                          )
                        }
                      />
                      PK
                    </label>
                    <button
                      type="button"
                      className="text-rose-300/90"
                      disabled={simpleCols.length <= 1}
                      onClick={() => setSimpleCols((prev) => prev.filter((x) => x.id !== c.id))}
                    >
                      ×
                    </button>
                  </div>
                ))}
                <button
                  type="button"
                  className="mt-1 rounded border border-border-subtle bg-black/25 px-2 py-0.5 text-[10px]"
                  onClick={() =>
                    setSimpleCols((prev) => [
                      ...prev,
                      {
                        id: nextColId(),
                        name: "col",
                        dataType: "text",
                        varcharLen: "",
                        not_null: false,
                        primary_key: false,
                      },
                    ])
                  }
                >
                  {t("db.createTableColAdd")}
                </button>
              </div>
              <button
                type="button"
                disabled={busy}
                onClick={() => void runSimpleCreateTable()}
                className="rounded border border-amber-800/50 bg-amber-950/35 px-2 py-1 text-[11px] text-amber-100"
              >
                {t("db.createTableSubmitSimple")}
              </button>
            </div>
          ) : (
            <div className="space-y-1">
              <textarea
                className="min-h-[6rem] w-full rounded border border-border-subtle bg-black/20 p-1 font-mono text-[10px] text-slate-200"
                value={createTableJson}
                onChange={(e) => setCreateTableJson(e.target.value)}
                spellCheck={false}
              />
              <button
                type="button"
                disabled={busy}
                onClick={() => void submitCreateTableJson(createTableJson)}
                className="rounded border border-amber-800/45 bg-amber-950/25 px-2 py-1 text-[11px] text-amber-100"
              >
                {t("db.v2AdminSubmitTable")}
              </button>
            </div>
          )}
          {adminTableResult ? (
            <pre className="max-h-24 overflow-auto rounded border border-border-subtle/50 bg-black/20 p-1.5 text-[10px] text-slate-400">
              {adminTableResult}
            </pre>
          ) : null}
        </div>
      ) : null}

      {showMigrationProbe ? (
        <div className="space-y-2 border-b border-border-subtle/40 pb-3">
          <div className="text-[10px] font-medium text-slate-400">{t("db.v2MigrationStatus")}</div>
          <div className="flex flex-wrap items-end gap-2 text-[10px]">
            <label className="text-slate-400">
              {t("db.v2MigrationDbName")}
              <input
                className="ml-1 rounded border border-border-subtle bg-black/25 px-1.5 py-0.5 font-mono text-slate-200"
                value={migrationDb}
                onChange={(e) => setMigrationDb(e.target.value)}
              />
            </label>
            <label className="text-slate-400">
              {t("db.serverMigrationToolsFilter")}
              <select
                className="ml-1 block rounded border border-border-subtle bg-black/25 px-1 py-0.5 font-mono text-slate-200"
                value={migrationToolsFilter}
                onChange={(e) => setMigrationToolsFilter(e.target.value as "all" | "core")}
              >
                <option value="core">{t("db.serverMigrationToolsCore")}</option>
                <option value="all">{t("db.serverMigrationToolsAll")}</option>
              </select>
            </label>
            <button
              type="button"
              disabled={busy || !migrationDb.trim()}
              onClick={() => void loadMigrationStatus()}
              className="rounded border border-cyan-800/50 bg-cyan-950/35 px-2 py-1 text-[11px] text-cyan-100"
            >
              {t("db.v2MigrationLoad")}
            </button>
          </div>
          {migrationParsed?.tools?.length ? (
            <div className="max-h-48 overflow-auto rounded border border-border-subtle/50">
              <table className="w-full border-collapse text-left text-[10px]">
                <thead>
                  <tr className="border-b border-border-subtle bg-black/30 text-slate-500">
                    <th className="px-2 py-1">{t("db.serverColTool")}</th>
                    <th className="px-2 py-1">{t("db.serverColPresent")}</th>
                    <th className="px-2 py-1">{t("db.serverColVersion")}</th>
                    <th className="px-2 py-1">{t("db.serverColNote")}</th>
                  </tr>
                </thead>
                <tbody>
                  {migrationParsed.tools.map((row, i) => (
                    <tr key={`${row.tool ?? i}-${i}`} className="border-b border-border-subtle/30">
                      <td className="px-2 py-1 font-mono text-slate-300">{row.tool ?? "—"}</td>
                      <td className="px-2 py-1">{row.present ? "✓" : "—"}</td>
                      <td className="max-w-[10rem] truncate px-2 py-1 font-mono text-amber-200/80">
                        {row.current_version ?? row.summary ?? "—"}
                      </td>
                      <td className="max-w-[14rem] truncate px-2 py-1 text-slate-500">
                        {row.error ?? row.summary ?? "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}
          {migrationRaw && !migrationParsed?.tools?.length ? (
            <pre className="max-h-32 overflow-auto rounded border border-border-subtle/50 bg-black/20 p-2 text-[10px] text-slate-400">
              {migrationRaw.length > 4000 ? `${migrationRaw.slice(0, 4000)}…` : migrationRaw}
            </pre>
          ) : null}
        </div>
      ) : caps.migration_status && !canBrowse ? (
        <p className="text-[10px] text-amber-200/70">{t("db.serverMigrationNeedsCreds")}</p>
      ) : null}

      {showMigrationRun ? (
        <div className="space-y-2 border-b border-border-subtle/40 pb-3">
          <div className="text-[10px] font-medium text-slate-400">{t("db.v2MigrationRun")}</div>
          <div className="flex flex-wrap items-end gap-2 text-[10px]">
            <label className="text-slate-400">
              {t("db.v2Tool")}
              <select
                className="ml-1 block rounded border border-border-subtle bg-black/25 px-1 py-0.5 font-mono text-slate-200"
                value={runTool}
                onChange={(e) => setRunTool(e.target.value)}
              >
                <option value="alembic">alembic</option>
                <option value="prisma">prisma</option>
                <option value="flyway">flyway</option>
              </select>
            </label>
            <label className="min-w-[12rem] flex-1 text-slate-400">
              {t("db.v2Workdir")}
              <input
                className="ml-1 block w-full rounded border border-border-subtle bg-black/25 px-1 py-0.5 font-mono text-slate-200"
                value={runWorkdir}
                onChange={(e) => setRunWorkdir(e.target.value)}
                placeholder="/var/app"
              />
            </label>
            <button
              type="button"
              disabled={busy || !runWorkdir.trim()}
              onClick={async () => {
                setRunResult(null);
                setBusy(true);
                try {
                  const j = await invoke<string>("control_api_host_db_v2_migration_run_json", {
                    instanceId,
                    tool: runTool,
                    workdir: runWorkdir.trim(),
                  });
                  setRunResult(j);
                } catch (e) {
                  setRunResult(e instanceof Error ? e.message : String(e));
                } finally {
                  setBusy(false);
                }
              }}
              className="rounded border border-amber-800/50 bg-amber-950/30 px-2 py-1 text-[11px] text-amber-100"
            >
              {t("db.v2MigrationRunDo")}
            </button>
          </div>
          {runResult ? (
            <pre className="max-h-40 overflow-auto text-[10px] text-slate-400">
              {runResult.length > 6000 ? `${runResult.slice(0, 6000)}…` : runResult}
            </pre>
          ) : null}
          <p className="text-[9px] text-slate-500">{t("db.serverMigrationRunHint")}</p>
        </div>
      ) : null}

      {showAdvancedUserOnly ? (
        <details className="space-y-2 text-[10px]">
          <summary className="cursor-pointer font-medium text-slate-400">{t("db.serverAdvancedAdmin")}</summary>
          <div className="space-y-2 pt-1">
            <div className="text-slate-500">{t("db.v2AdminCreateUser")}</div>
            <p className="text-[9px] text-slate-500">{t("db.serverCreateUserCredsHint")}</p>
            <div className="grid max-w-2xl grid-cols-1 gap-2 sm:grid-cols-2">
              <label className="block text-slate-400">
                {t("db.inspectorCuDatabase")}
                <input
                  className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/20 px-1.5 py-0.5 font-mono text-slate-200"
                  value={cuDatabase}
                  onChange={(e) => setCuDatabase(e.target.value)}
                  placeholder="postgres"
                  spellCheck={false}
                />
              </label>
              <label className="block text-slate-400">
                {t("db.inspectorCuUsername")}
                <input
                  className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/20 px-1.5 py-0.5 font-mono text-slate-200"
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
                    className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/20 px-1.5 py-0.5 font-mono"
                    value={cuPassword}
                    onChange={(e) => setCuPassword(e.target.value)}
                    autoComplete="new-password"
                  />
                </label>
              ) : null}
              <label className="block text-slate-400">
                {t("db.inspectorCuPrivileges")}
                <select
                  className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/20 px-1.5 py-0.5 font-mono text-slate-200"
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
                      className="mt-0.5 block w-full max-w-xs rounded border border-border-subtle bg-black/20 px-1.5 py-0.5 font-mono text-slate-200"
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
              <pre className="max-h-32 overflow-auto whitespace-pre-wrap rounded border border-border-subtle/50 bg-black/30 p-2 font-mono text-[9px] text-slate-400">
                {createUserRequestJson}
              </pre>
            </div>
            <button
              type="button"
              disabled={busy || !cuUsername.trim()}
              onClick={async () => {
                setAdminUserResult(null);
                setBusy(true);
                try {
                  const j = await invoke<string>("control_api_host_db_v2_admin_create_user_json", {
                    instanceId,
                    bodyJson: createUserRequestJson,
                    ...dbCredsInvoke(),
                  });
                  setAdminUserResult(j);
                } catch (e) {
                  setAdminUserResult(e instanceof Error ? e.message : String(e));
                } finally {
                  setBusy(false);
                }
              }}
              className="rounded border border-amber-800/45 bg-amber-950/25 px-2 py-1 text-[11px] text-amber-100"
            >
              {t("db.v2AdminSubmitUser")}
            </button>
            {adminUserResult ? (
              <pre className="max-h-32 overflow-auto text-[10px] text-amber-200/85">{adminUserResult}</pre>
            ) : null}
          </div>
          <p className="text-[9px] text-slate-500">
            {language === "ru"
              ? "Создание таблицы — в секции выше; учётка из панели «Базы» передаётся в запрос. Только PostgreSQL (admin API)."
              : "Create table is in the section above; DB creds from the Databases panel are sent with the request. PostgreSQL-only (admin API)."}
          </p>
        </details>
      ) : null}

      {busy ? <p className="text-[9px] text-slate-500">…</p> : null}
    </div>
  );
}
