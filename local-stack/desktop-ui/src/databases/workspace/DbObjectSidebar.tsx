import { ChevronRight, Database, RefreshCw, Search } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../../i18n";
import { hostDbSchemas, hostDbTables, type DbCredsInvoke } from "./hostDbApi";
import { useHostDbWorkspaceStore } from "./hostDbWorkspaceStore";

type Props = {
  instanceId: string;
  dbCredsInvoke: DbCredsInvoke;
  canBrowse: boolean;
};

export function DbObjectSidebar({ instanceId, dbCredsInvoke, canBrowse }: Props) {
  const { t } = useI18n();
  const {
    sidebarSearch,
    setSidebarSearch,
    expandedKeys,
    toggleExpandedKey,
    openTableDataTab,
    openTableSchemaTab,
    logAction,
  } = useHostDbWorkspaceStore();

  const [schemas, setSchemas] = useState<string[]>([]);
  const [tablesBySchema, setTablesBySchema] = useState<Record<string, { name: string }[]>>({});
  const [loadingSchemas, setLoadingSchemas] = useState(false);
  const [loadingTablesFor, setLoadingTablesFor] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const tablesLoadedRef = useRef<Set<string>>(new Set());

  const loadSchemas = useCallback(async () => {
    if (!canBrowse) return;
    setErr(null);
    setLoadingSchemas(true);
    try {
      const list = await hostDbSchemas(instanceId, dbCredsInvoke);
      setSchemas(list);
      tablesLoadedRef.current = new Set();
      setTablesBySchema({});
      logAction("tree.schemas", `${list.length} schemas`);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setSchemas([]);
    } finally {
      setLoadingSchemas(false);
    }
  }, [canBrowse, instanceId, dbCredsInvoke, logAction]);

  useEffect(() => {
    void loadSchemas();
  }, [loadSchemas]);

  const ensureTables = useCallback(
    async (schema: string) => {
      if (tablesLoadedRef.current.has(schema)) return;
      tablesLoadedRef.current.add(schema);
      setLoadingTablesFor(schema);
      setErr(null);
      try {
        const tables = await hostDbTables(instanceId, schema, dbCredsInvoke);
        setTablesBySchema((prev) => ({
          ...prev,
          [schema]: tables.map((x) => ({ name: x.name })),
        }));
        logAction("tree.tables", `${schema}: ${tables.length}`);
      } catch (e) {
        tablesLoadedRef.current.delete(schema);
        setErr(e instanceof Error ? e.message : String(e));
      } finally {
        setLoadingTablesFor(null);
      }
    },
    [instanceId, dbCredsInvoke, logAction],
  );

  const onToggleSchema = (schema: string) => {
    const key = `schema:${schema}`;
    const willExpand = !expandedKeys.includes(key);
    toggleExpandedKey(key);
    if (willExpand) void ensureTables(schema);
  };

  const searchLo = sidebarSearch.trim().toLowerCase();
  const filteredSchemas = useMemo(() => {
    if (!searchLo) return schemas;
    return schemas.filter((s) => s.toLowerCase().includes(searchLo));
  }, [schemas, searchLo]);

  const tableClick = (schema: string, table: string, e: React.MouseEvent) => {
    const bg = e.metaKey || e.ctrlKey;
    openTableDataTab(instanceId, schema, table, !bg);
  };

  const tableDblClick = (schema: string, table: string) => {
    openTableDataTab(instanceId, schema, table, true);
  };

  const openSchemaTab = (schema: string, table: string, e: React.MouseEvent) => {
    e.stopPropagation();
    const bg = e.metaKey || e.ctrlKey;
    openTableSchemaTab(instanceId, schema, table, !bg);
  };

  return (
    <div className="flex h-full min-h-0 w-full max-w-full flex-1 flex-col border-r border-red-900/25 bg-black/25 lg:max-w-[14rem] lg:min-w-[10.5rem]">
      <div className="flex shrink-0 items-center gap-1 border-b border-red-900/20 px-2 py-1.5">
        <Database className="h-3.5 w-3.5 shrink-0 text-red-400/90" />
        <span className="text-[10px] font-semibold uppercase tracking-wide text-slate-500">
          {t("db.workspace.objects")}
        </span>
      </div>
      <div className="relative shrink-0 border-b border-white/5 px-2 py-1.5">
        <Search className="pointer-events-none absolute left-4 top-1/2 h-3 w-3 -translate-y-1/2 text-slate-600" />
        <input
          type="search"
          value={sidebarSearch}
          onChange={(e) => setSidebarSearch(e.target.value)}
          placeholder={t("db.workspace.search")}
          className="w-full rounded border border-red-900/20 bg-black/30 py-1 pl-7 pr-2 text-[10px] text-slate-200 placeholder:text-slate-600"
        />
      </div>
      <div className="flex shrink-0 items-center gap-1 border-b border-white/5 px-2 py-1">
        <button
          type="button"
          disabled={!canBrowse || loadingSchemas}
          onClick={() => void loadSchemas()}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-black/30 px-1.5 py-0.5 text-[10px] text-slate-300 hover:bg-black/45 disabled:opacity-50"
        >
          <RefreshCw className={`h-3 w-3 ${loadingSchemas ? "animate-spin" : ""}`} />
          {t("db.loadMeta")}
        </button>
      </div>
      {err ? <p className="shrink-0 px-2 py-1 text-[10px] text-rose-300">{err}</p> : null}
      <div className="min-h-0 flex-1 overflow-auto py-1 text-[10px]">
        {!canBrowse ? (
          <p className="px-2 text-slate-600">{t("db.authBlocked")}</p>
        ) : (
          filteredSchemas.map((sch) => {
            const key = `schema:${sch}`;
            const open = expandedKeys.includes(key);
            const tables = tablesBySchema[sch] ?? [];
            const loadingT = loadingTablesFor === sch;
            const schMatch = !searchLo || sch.toLowerCase().includes(searchLo);
            const tablesFiltered =
              searchLo && schMatch
                ? tables.filter((tb) => tb.name.toLowerCase().includes(searchLo))
                : tables;
            return (
              <div key={sch} className="mb-0.5">
                <button
                  type="button"
                  onClick={() => onToggleSchema(sch)}
                  className="flex w-full items-center gap-0.5 rounded px-1.5 py-0.5 text-left text-slate-400 hover:bg-white/5 hover:text-slate-200"
                >
                  <ChevronRight className={`h-3 w-3 shrink-0 transition-transform ${open ? "rotate-90" : ""}`} />
                  <span className="truncate font-mono text-slate-300">{sch}</span>
                  {loadingT ? <span className="text-[9px] text-slate-600">…</span> : null}
                </button>
                {open ? (
                  <div className="ml-3 border-l border-white/[0.06] pl-1">
                    {tablesFiltered.map((tb) => (
                      <div key={tb.name} className="group flex items-center gap-0.5">
                        <button
                          type="button"
                          onClick={(e) => tableClick(sch, tb.name, e)}
                          onDoubleClick={() => tableDblClick(sch, tb.name)}
                          className="min-w-0 flex-1 truncate rounded px-1 py-0.5 text-left font-mono text-slate-400 hover:bg-red-950/25 hover:text-amber-100/90"
                        >
                          {tb.name}
                        </button>
                        <button
                          type="button"
                          title={t("db.workspace.openStructure")}
                          onClick={(e) => openSchemaTab(sch, tb.name, e)}
                          className="shrink-0 rounded px-0.5 text-[9px] text-violet-400/80 opacity-0 hover:bg-violet-950/30 group-hover:opacity-100"
                        >
                          Σ
                        </button>
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
