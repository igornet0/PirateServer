import { Columns2, History, Plus, Shield } from "lucide-react";
import React, { useEffect, useMemo, useRef } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import { useI18n } from "../../i18n";
import { ModalDialog } from "../../ui/ModalDialog";
import { DbContextPanel } from "./DbContextPanel";
import { DbDrawer } from "./DbDrawer";
import { DbObjectSidebar } from "./DbObjectSidebar";
import { DbTabWorkspace } from "./DbTabWorkspace";
import { DbTabsBar } from "./DbTabsBar";
import { hostDbRelationshipsJson, type DbCredsInvoke } from "./hostDbApi";
import { useHostDbWorkspaceStore } from "./hostDbWorkspaceStore";
import { useViewportMinLg } from "./useViewportMinLg";

type Props = {
  instanceId: string;
  engine: string;
  canBrowse: boolean;
  canRunReadonlySql: boolean;
  dbCredsInvoke: DbCredsInvoke;
};

type MobilePane = "tree" | "work" | "context";

export function HostDbWorkspace({ instanceId, engine, canBrowse, canRunReadonlySql, dbCredsInvoke }: Props) {
  const { t } = useI18n();
  const persistTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [fkGlobal, setFkGlobal] = React.useState<string | null>(null);
  const isLg = useViewportMinLg();
  const [mobilePane, setMobilePane] = React.useState<MobilePane>("work");

  const {
    hydrateFromStorage,
    persistToStorage,
    setInstanceId,
    tabs,
    activeTabId,
    secondaryTabId,
    splitEnabled,
    setSplitEnabled,
    setSecondaryTabId,
    activateTab,
    closeTab,
    pinTab,
    duplicateTab,
    reorderTabs,
    closeOtherTabs,
    openSqlTab,
    openAdminTab,
    rightPanelOpen,
    setRightPanelOpen,
    contextTab,
    setContextTab,
    livePollSec,
    setLivePollSec,
    drawerOpen,
    drawerTitle,
    drawerBody,
    closeDrawer,
    openDrawer,
    confirmModal,
    closeConfirm,
    actionLog,
    logAction,
    sidebarSearch,
    expandedKeys,
    sqlTabCounter,
  } = useHostDbWorkspaceStore();

  useEffect(() => {
    setInstanceId(instanceId);
    hydrateFromStorage(instanceId);
    return () => {
      persistToStorage(instanceId);
    };
  }, [instanceId, hydrateFromStorage, persistToStorage, setInstanceId]);

  useEffect(() => {
    if (persistTimer.current) clearTimeout(persistTimer.current);
    persistTimer.current = setTimeout(() => {
      persistToStorage(instanceId);
    }, 400);
    return () => {
      if (persistTimer.current) clearTimeout(persistTimer.current);
    };
  }, [
    instanceId,
    tabs,
    activeTabId,
    secondaryTabId,
    splitEnabled,
    persistToStorage,
    sidebarSearch,
    expandedKeys,
    rightPanelOpen,
    contextTab,
    sqlTabCounter,
    livePollSec,
  ]);

  useEffect(() => {
    if (contextTab !== "grants") {
      setFkGlobal(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const j = await hostDbRelationshipsJson(instanceId, dbCredsInvoke);
        if (!cancelled) setFkGlobal(j);
      } catch {
        if (!cancelled) setFkGlobal(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [contextTab, instanceId, dbCredsInvoke]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.key !== "w") return;
      const target = e.target as HTMLElement;
      if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) return;
      if (!activeTabId) return;
      e.preventDefault();
      closeTab(activeTabId);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeTabId, closeTab]);

  useEffect(() => {
    if (!isLg && splitEnabled) {
      setSplitEnabled(false);
    }
  }, [isLg, splitEnabled, setSplitEnabled]);

  useEffect(() => {
    setMobilePane("work");
  }, [instanceId]);

  const activeTab = useMemo(
    () => tabs.find((x) => x.id === activeTabId) ?? null,
    [tabs, activeTabId],
  );

  const secondaryTab = useMemo(
    () => (secondaryTabId ? tabs.find((x) => x.id === secondaryTabId) ?? null : null),
    [tabs, secondaryTabId],
  );

  const focusSchemaForInspector = useMemo(() => {
    if (!activeTab) return "";
    if (activeTab.kind === "table_data" || activeTab.kind === "table_schema") return activeTab.schema;
    return "";
  }, [activeTab]);

  const fkForContext =
    activeTab?.kind === "table_schema" && activeTab.data.fkJson ? activeTab.data.fkJson : fkGlobal;

  const tabBarItems = tabs.map((x) => ({
    id: x.id,
    title: x.title,
    pinned: x.pinned,
    kind: x.kind,
  }));

  const toolbarExtras = (
    <div className="flex shrink-0 items-center gap-0.5 pr-1">
      <button
        type="button"
        title={t("db.workspace.newSql")}
        onClick={() => openSqlTab(instanceId, true)}
        className="touch-manipulation rounded p-1.5 text-slate-500 hover:bg-white/10 hover:text-amber-200/90 lg:p-1"
      >
        <Plus className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        title={t("db.workspace.admin")}
        onClick={() => openAdminTab(instanceId, true)}
        className="touch-manipulation rounded p-1.5 text-slate-500 hover:bg-white/10 hover:text-violet-200/90 lg:p-1"
      >
        <Shield className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        title={t("db.workspace.split")}
        onClick={() => {
          setSplitEnabled(!splitEnabled);
          logAction("layout.split", String(!splitEnabled));
        }}
        className={`hidden touch-manipulation rounded p-1 lg:inline-flex ${splitEnabled ? "bg-red-950/50 text-amber-100" : "text-slate-500 hover:bg-white/10"}`}
      >
        <Columns2 className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        title={t("db.workspace.actionLog")}
        onClick={() =>
          openDrawer(
            t("db.workspace.actionLog"),
            actionLog.length === 0
              ? "—"
              : actionLog
                  .slice(0, 80)
                  .map((a) => `${new Date(a.t).toISOString()}  ${a.type}  ${a.detail}`)
                  .join("\n"),
          )
        }
        className="touch-manipulation rounded p-1.5 text-slate-500 hover:bg-white/10 lg:p-1"
      >
        <History className="h-3.5 w-3.5" />
      </button>
      {activeTabId ? (
        <button
          type="button"
          className="hidden touch-manipulation rounded px-1 text-[9px] text-slate-600 hover:text-slate-400 lg:inline"
          onClick={() => closeOtherTabs(activeTabId)}
        >
          {t("db.workspace.closeOthers")}
        </button>
      ) : null}
    </div>
  );

  const workArea = (
    <>
      <DbTabsBar
        tabs={tabBarItems}
        activeId={activeTabId}
        onActivate={activateTab}
        onClose={closeTab}
        onPin={pinTab}
        onDuplicate={duplicateTab}
        onReorder={reorderTabs}
        extraLeading={toolbarExtras}
      />
      {splitEnabled ? (
        <div className="hidden shrink-0 items-center gap-2 border-b border-white/5 px-2 py-1 text-[10px] text-slate-500 lg:flex">
          <span>{t("db.workspace.secondPane")}</span>
          <select
            className="max-w-[12rem] rounded border border-red-900/30 bg-black/40 px-1 py-0.5 font-mono text-[10px] text-slate-200"
            value={secondaryTabId ?? ""}
            onChange={(e) => setSecondaryTabId(e.target.value || null)}
          >
            <option value="">—</option>
            {tabs
              .filter((x) => x.id !== activeTabId)
              .map((x) => (
                <option key={x.id} value={x.id}>
                  {x.title}
                </option>
              ))}
          </select>
        </div>
      ) : null}
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-white/5 px-2 py-1.5 text-[10px] text-slate-500 lg:py-1">
        <label className="flex items-center gap-1">
          {t("db.workspace.livePoll")}
          <select
            className="min-h-[36px] rounded border border-red-900/25 bg-black/40 px-1 py-1 text-[10px] lg:min-h-0 lg:py-0.5"
            value={livePollSec}
            onChange={(e) => setLivePollSec(Number(e.target.value))}
          >
            <option value={0}>off</option>
            <option value={30}>30s</option>
            <option value={60}>60s</option>
          </select>
        </label>
        <button
          type="button"
          onClick={() => setRightPanelOpen(!rightPanelOpen)}
          className="hidden min-h-[36px] text-slate-600 hover:text-slate-400 lg:inline"
        >
          {rightPanelOpen ? t("db.workspace.hideContext") : t("db.workspace.showContext")}
        </button>
      </div>
      <DbTabWorkspace
        tab={activeTab}
        secondaryTab={secondaryTab}
        splitEnabled={splitEnabled}
        instanceId={instanceId}
        engine={engine}
        dbCredsInvoke={dbCredsInvoke}
        canRunReadonlySql={canRunReadonlySql}
        focusSchemaForInspector={focusSchemaForInspector}
      />
      <DbDrawer open={drawerOpen} title={drawerTitle} onClose={closeDrawer}>
        <pre className="whitespace-pre-wrap break-all font-mono text-[10px] text-slate-300">{drawerBody}</pre>
      </DbDrawer>
    </>
  );

  const contextArea = (
    <DbContextPanel
      tab={activeTab}
      fkJson={fkForContext}
      onOpenAdmin={() => openAdminTab(instanceId, true)}
      contextTab={contextTab}
      onContextTab={setContextTab}
    />
  );

  return (
    <div className="relative flex min-h-[min(24rem,70vh)] min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-red-900/25 bg-black/15 lg:min-h-[28rem]">
      {!isLg ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <div
            className="flex shrink-0 border-b border-red-900/30 bg-black/30"
            role="tablist"
            aria-label={t("db.workspace.mobileWork")}
          >
            {(
              [
                ["tree", t("db.workspace.mobileObjects")] as const,
                ["work", t("db.workspace.mobileWork")] as const,
                ["context", t("db.workspace.mobileContext")] as const,
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={mobilePane === id}
                onClick={() => setMobilePane(id)}
                className={`touch-manipulation flex-1 px-1 py-3 text-[10px] font-medium sm:text-[11px] ${
                  mobilePane === id ? "border-b-2 border-red-500 text-amber-100" : "text-slate-500"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
          <div className="relative min-h-0 flex-1">
            {mobilePane === "tree" ? (
              <DbObjectSidebar
                key={instanceId}
                instanceId={instanceId}
                dbCredsInvoke={dbCredsInvoke}
                canBrowse={canBrowse}
              />
            ) : null}
            {mobilePane === "work" ? <div className="flex h-full min-h-0 flex-col">{workArea}</div> : null}
            {mobilePane === "context" ? <div className="h-full min-h-0 overflow-auto">{contextArea}</div> : null}
          </div>
        </div>
      ) : (
        <Group orientation="horizontal" className="min-h-0 flex-1">
          <Panel defaultSize={20} minSize={14} className="min-h-0 min-w-0">
            <DbObjectSidebar
              key={instanceId}
              instanceId={instanceId}
              dbCredsInvoke={dbCredsInvoke}
              canBrowse={canBrowse}
            />
          </Panel>
          <Separator className="w-1 bg-red-950/30 hover:bg-red-800/50" />
          <Panel defaultSize={rightPanelOpen ? 62 : 78} minSize={35} className="relative flex min-h-0 min-w-0 flex-col">
            {workArea}
          </Panel>
          {rightPanelOpen ? (
            <>
              <Separator className="w-1 bg-red-950/25 hover:bg-red-800/45" />
              <Panel defaultSize={18} minSize={12} maxSize={28} className="min-h-0 min-w-0">
                {contextArea}
              </Panel>
            </>
          ) : null}
        </Group>
      )}

      <ModalDialog
        open={confirmModal.open}
        onClose={closeConfirm}
        zClassName="z-modalConfirm"
        panelClassName="w-full max-w-md"
        className="flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
      >
        <div className="rounded-lg border border-border-subtle bg-panel p-4 shadow-card">
          <p className="text-sm text-slate-200">{confirmModal.message}</p>
          <div className="mt-3 flex justify-end gap-2">
            <button
              type="button"
              className="rounded border border-border-subtle px-3 py-1.5 text-xs text-slate-300"
              onClick={closeConfirm}
            >
              {t("storage.modalCancel")}
            </button>
            <button
              type="button"
              className="rounded border border-amber-800/50 bg-amber-950/35 px-3 py-1.5 text-xs text-amber-100"
              onClick={() => {
                confirmModal.onConfirm?.();
                closeConfirm();
              }}
            >
              {t("storage.modalSave")}
            </button>
          </div>
        </div>
      </ModalDialog>
    </div>
  );
}
