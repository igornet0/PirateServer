import { create } from "zustand";
import type { HostQueryResult } from "./hostDbApi";

const PERSIST_VER = 1;
export const hostDbWorkspacePersistKey = (instanceId: string) =>
  `pirate.hostDb.workspace.v${PERSIST_VER}:${instanceId}`;

export type TableDataTabState = {
  status: "idle" | "loading" | "error";
  error: string | null;
  columns: { name: string; type: string }[];
  rows: unknown[];
  offset: number;
  pageSize: number;
  truncated: boolean;
  warn: string | null;
  sortColumn: string | null;
  sortDesc: boolean;
  filterColumn: string | null;
  filterValue: string;
};

export type SchemaTabState = {
  status: "idle" | "loading" | "error";
  error: string | null;
  columns: { name: string; type: string }[];
  fkJson: string | null;
  fkStatus: "idle" | "loading" | "error";
  fkError: string | null;
};

export type SqlTabState = {
  sql: string;
  status: "idle" | "loading" | "error";
  error: string | null;
  result: HostQueryResult | null;
};

export type AdminTabState = Record<string, never>;

export type TabKind = "table_data" | "table_schema" | "sql" | "admin";

type TabBase = {
  id: string;
  pinned: boolean;
  title: string;
  instanceId: string;
};

export type TableDataTab = TabBase & {
  kind: "table_data";
  schema: string;
  table: string;
  data: TableDataTabState;
};

export type TableSchemaTab = TabBase & {
  kind: "table_schema";
  schema: string;
  table: string;
  data: SchemaTabState;
};

export type SqlWorkspaceTab = TabBase & {
  kind: "sql";
  data: SqlTabState;
};

export type AdminWorkspaceTab = TabBase & {
  kind: "admin";
  data: AdminTabState;
};

export type WorkspaceTab = TableDataTab | TableSchemaTab | SqlWorkspaceTab | AdminWorkspaceTab;

export type ActionLogEntry = { t: number; type: string; detail: string };

export type ContextPanelTab = "properties" | "indexes" | "grants";

type PersistedTab = {
  id: string;
  kind: TabKind;
  pinned: boolean;
  title: string;
  instanceId: string;
  schema?: string;
  table?: string;
  sql?: string;
};

type PersistedShape = {
  activeTabId: string | null;
  secondaryTabId: string | null;
  splitEnabled: boolean;
  sidebarSearch: string;
  expandedKeys: string[];
  rightPanelOpen: boolean;
  contextTab: ContextPanelTab;
  tabs: PersistedTab[];
  sqlTabCounter: number;
  livePollSec: number;
};

function emptyTableDataState(pageSize = 100): TableDataTabState {
  return {
    status: "idle",
    error: null,
    columns: [],
    rows: [],
    offset: 0,
    pageSize,
    truncated: false,
    warn: null,
    sortColumn: null,
    sortDesc: false,
    filterColumn: null,
    filterValue: "",
  };
}

function emptySchemaState(): SchemaTabState {
  return {
    status: "idle",
    error: null,
    columns: [],
    fkJson: null,
    fkStatus: "idle",
    fkError: null,
  };
}

function emptySqlState(sql: string): SqlTabState {
  return {
    sql,
    status: "idle",
    error: null,
    result: null,
  };
}

function hydrateTab(p: PersistedTab): WorkspaceTab | null {
  if (p.instanceId == null) return null;
  switch (p.kind) {
    case "table_data":
      if (!p.schema || !p.table) return null;
      return {
        id: p.id,
        pinned: p.pinned,
        title: p.title,
        instanceId: p.instanceId,
        kind: "table_data",
        schema: p.schema,
        table: p.table,
        data: emptyTableDataState(),
      };
    case "table_schema":
      if (!p.schema || !p.table) return null;
      return {
        id: p.id,
        pinned: p.pinned,
        title: p.title,
        instanceId: p.instanceId,
        kind: "table_schema",
        schema: p.schema,
        table: p.table,
        data: emptySchemaState(),
      };
    case "sql":
      return {
        id: p.id,
        pinned: p.pinned,
        title: p.title,
        instanceId: p.instanceId,
        kind: "sql",
        data: emptySqlState(p.sql ?? "SELECT 1"),
      };
    case "admin":
      return {
        id: p.id,
        pinned: p.pinned,
        title: p.title,
        instanceId: p.instanceId,
        kind: "admin",
        data: {},
      };
    default:
      return null;
  }
}

function serializeTabs(tabs: WorkspaceTab[]): PersistedTab[] {
  return tabs.map((t) => {
    const base: PersistedTab = {
      id: t.id,
      kind: t.kind,
      pinned: t.pinned,
      title: t.title,
      instanceId: t.instanceId,
    };
    if (t.kind === "table_data" || t.kind === "table_schema") {
      base.schema = t.schema;
      base.table = t.table;
    }
    if (t.kind === "sql") {
      base.sql = t.data.sql;
    }
    return base;
  });
}

type Store = {
  /** Active host instance this workspace is bound to */
  instanceId: string | null;
  tabs: WorkspaceTab[];
  activeTabId: string | null;
  secondaryTabId: string | null;
  splitEnabled: boolean;
  sidebarSearch: string;
  expandedKeys: string[];
  rightPanelOpen: boolean;
  contextTab: ContextPanelTab;
  actionLog: ActionLogEntry[];
  sqlTabCounter: number;
  /** Seconds between refetches for the active Data tab; 0 = off. Live push needs a future control-api channel. */
  livePollSec: number;
  drawerOpen: boolean;
  drawerTitle: string;
  drawerBody: string | null;
  confirmModal: { open: boolean; message: string; onConfirm: (() => void) | null };

  setInstanceId: (id: string | null) => void;
  hydrateFromStorage: (instanceId: string) => void;
  persistToStorage: (instanceId: string) => void;
  logAction: (type: string, detail: string) => void;

  setSidebarSearch: (s: string) => void;
  setExpandedKeys: (keys: string[]) => void;
  toggleExpandedKey: (key: string) => void;
  setRightPanelOpen: (o: boolean) => void;
  setContextTab: (t: ContextPanelTab) => void;
  setSplitEnabled: (v: boolean) => void;
  setSecondaryTabId: (id: string | null) => void;
  setLivePollSec: (s: number) => void;

  openDrawer: (title: string, body: string) => void;
  closeDrawer: () => void;
  openConfirm: (message: string, onConfirm: () => void) => void;
  closeConfirm: () => void;

  activateTab: (id: string) => void;
  closeTab: (id: string) => void;
  pinTab: (id: string) => void;
  duplicateTab: (id: string) => void;
  reorderTabs: (fromIndex: number, toIndex: number) => void;
  closeOtherTabs: (keepId: string) => void;

  openTableDataTab: (instanceId: string, schema: string, table: string, activate: boolean) => void;
  openTableSchemaTab: (instanceId: string, schema: string, table: string, activate: boolean) => void;
  openSqlTab: (instanceId: string, activate: boolean) => void;
  openAdminTab: (instanceId: string, activate: boolean) => void;

  updateTableData: (tabId: string, patch: Partial<TableDataTabState>) => void;
  updateSchemaTab: (tabId: string, patch: Partial<SchemaTabState>) => void;
  updateSqlTab: (tabId: string, patch: Partial<SqlTabState>) => void;
};

function newId(): string {
  return crypto.randomUUID?.() ?? `tab-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export const useHostDbWorkspaceStore = create<Store>((set, get) => ({
  instanceId: null,
  tabs: [],
  activeTabId: null,
  secondaryTabId: null,
  splitEnabled: false,
  sidebarSearch: "",
  expandedKeys: [],
  rightPanelOpen: true,
  contextTab: "properties",
  actionLog: [],
  sqlTabCounter: 0,
  livePollSec: 0,
  drawerOpen: false,
  drawerTitle: "",
  drawerBody: null,
  confirmModal: { open: false, message: "", onConfirm: null },

  setInstanceId: (instanceId) => set({ instanceId }),

  hydrateFromStorage: (instanceId) => {
    try {
      const raw = localStorage.getItem(hostDbWorkspacePersistKey(instanceId));
      if (!raw) {
        set({
          instanceId,
          tabs: [],
          activeTabId: null,
          secondaryTabId: null,
          splitEnabled: false,
          sidebarSearch: "",
          expandedKeys: [],
          rightPanelOpen: true,
          contextTab: "properties",
          sqlTabCounter: 0,
          livePollSec: 0,
        });
        return;
      }
      const p = JSON.parse(raw) as PersistedShape;
      const tabs = (p.tabs ?? []).map(hydrateTab).filter((x): x is WorkspaceTab => x != null);
      set({
        instanceId,
        tabs,
        activeTabId: p.activeTabId && tabs.some((t) => t.id === p.activeTabId) ? p.activeTabId : tabs[0]?.id ?? null,
        secondaryTabId:
          p.secondaryTabId && tabs.some((t) => t.id === p.secondaryTabId) ? p.secondaryTabId : null,
        splitEnabled: Boolean(p.splitEnabled),
        sidebarSearch: p.sidebarSearch ?? "",
        expandedKeys: Array.isArray(p.expandedKeys) ? p.expandedKeys : [],
        rightPanelOpen: p.rightPanelOpen !== false,
        contextTab: p.contextTab ?? "properties",
        sqlTabCounter: typeof p.sqlTabCounter === "number" ? p.sqlTabCounter : 0,
        livePollSec: typeof p.livePollSec === "number" ? p.livePollSec : 0,
      });
    } catch {
      set({ instanceId, tabs: [], activeTabId: null });
    }
  },

  persistToStorage: (instanceId) => {
    const s = get();
    if (s.tabs.length === 0 && !s.activeTabId) {
      try {
        localStorage.removeItem(hostDbWorkspacePersistKey(instanceId));
      } catch {
        /* ignore */
      }
      return;
    }
    const payload: PersistedShape = {
      activeTabId: s.activeTabId,
      secondaryTabId: s.secondaryTabId,
      splitEnabled: s.splitEnabled,
      sidebarSearch: s.sidebarSearch,
      expandedKeys: s.expandedKeys,
      rightPanelOpen: s.rightPanelOpen,
      contextTab: s.contextTab,
      tabs: serializeTabs(s.tabs),
      sqlTabCounter: s.sqlTabCounter,
      livePollSec: s.livePollSec,
    };
    try {
      localStorage.setItem(hostDbWorkspacePersistKey(instanceId), JSON.stringify(payload));
    } catch {
      /* ignore */
    }
  },

  logAction: (type, detail) =>
    set((st) => ({
      actionLog: [{ t: Date.now(), type, detail }, ...st.actionLog].slice(0, 200),
    })),

  setSidebarSearch: (sidebarSearch) => set({ sidebarSearch }),
  setExpandedKeys: (expandedKeys) => set({ expandedKeys }),
  toggleExpandedKey: (key) =>
    set((st) => ({
      expandedKeys: st.expandedKeys.includes(key)
        ? st.expandedKeys.filter((k) => k !== key)
        : [...st.expandedKeys, key],
    })),
  setRightPanelOpen: (rightPanelOpen) => set({ rightPanelOpen }),
  setContextTab: (contextTab) => set({ contextTab }),
  setSplitEnabled: (splitEnabled) => set({ splitEnabled }),
  setSecondaryTabId: (secondaryTabId) => set({ secondaryTabId }),
  setLivePollSec: (livePollSec) => set({ livePollSec }),

  openDrawer: (drawerTitle, drawerBody) => set({ drawerOpen: true, drawerTitle, drawerBody }),
  closeDrawer: () => set({ drawerOpen: false, drawerBody: null }),
  openConfirm: (message, onConfirm) =>
    set({ confirmModal: { open: true, message, onConfirm } }),
  closeConfirm: () => set({ confirmModal: { open: false, message: "", onConfirm: null } }),

  activateTab: (id) => set({ activeTabId: id }),

  closeTab: (id) =>
    set((st) => {
      const tab = st.tabs.find((t) => t.id === id);
      if (tab?.pinned) return st;
      const tabs = st.tabs.filter((t) => t.id !== id);
      let activeTabId = st.activeTabId;
      if (activeTabId === id) {
        const idx = st.tabs.findIndex((t) => t.id === id);
        const next = tabs[Math.max(0, idx - 1)] ?? tabs[0];
        activeTabId = next?.id ?? null;
      }
      let secondaryTabId = st.secondaryTabId;
      if (secondaryTabId === id) {
        secondaryTabId = null;
      }
      return { tabs, activeTabId, secondaryTabId };
    }),

  pinTab: (id) =>
    set((st) => ({
      tabs: st.tabs.map((t) => (t.id === id ? { ...t, pinned: !t.pinned } : t)),
    })),

  duplicateTab: (id) =>
    set((st) => {
      const tab = st.tabs.find((t) => t.id === id);
      if (!tab) return st;
      const nid = newId();
      let copy: WorkspaceTab;
      if (tab.kind === "table_data") {
        copy = {
          ...tab,
          id: nid,
          pinned: false,
          title: `${tab.title} (2)`,
          data: { ...emptyTableDataState(tab.data.pageSize) },
        };
      } else if (tab.kind === "table_schema") {
        copy = {
          ...tab,
          id: nid,
          pinned: false,
          title: `${tab.title} (2)`,
          data: emptySchemaState(),
        };
      } else if (tab.kind === "sql") {
        copy = {
          ...tab,
          id: nid,
          pinned: false,
          title: `${tab.title} (2)`,
          data: { ...emptySqlState(tab.data.sql) },
        };
      } else {
        copy = { ...tab, id: nid, pinned: false, title: `${tab.title} (2)`, data: {} };
      }
      return { tabs: [...st.tabs, copy], activeTabId: nid };
    }),

  reorderTabs: (fromIndex, toIndex) =>
    set((st) => {
      const tabs = [...st.tabs];
      const [m] = tabs.splice(fromIndex, 1);
      if (!m) return st;
      tabs.splice(toIndex, 0, m);
      return { tabs };
    }),

  closeOtherTabs: (keepId) =>
    set((st) => ({
      tabs: st.tabs.filter((t) => t.id === keepId || t.pinned),
      activeTabId: keepId,
      secondaryTabId: st.secondaryTabId === keepId ? st.secondaryTabId : null,
    })),

  openTableDataTab: (instanceId, schema, table, activate) => {
    const st = get();
    const exist = st.tabs.find(
      (t) => t.kind === "table_data" && t.schema === schema && t.table === table && t.instanceId === instanceId,
    );
    if (exist) {
      set({ activeTabId: exist.id });
      get().logAction("tab.activate", `table_data ${schema}.${table}`);
      return;
    }
    const id = newId();
    const tab: TableDataTab = {
      id,
      pinned: false,
      title: `${schema}.${table}`,
      instanceId,
      kind: "table_data",
      schema,
      table,
      data: emptyTableDataState(),
    };
    set((s) => ({
      tabs: [...s.tabs, tab],
      activeTabId: activate ? id : s.activeTabId,
    }));
    get().logAction("tab.open", `table_data ${schema}.${table}`);
  },

  openTableSchemaTab: (instanceId, schema, table, activate) => {
    const st = get();
    const exist = st.tabs.find(
      (t) =>
        t.kind === "table_schema" && t.schema === schema && t.table === table && t.instanceId === instanceId,
    );
    if (exist) {
      if (activate) set({ activeTabId: exist.id });
      return;
    }
    const id = newId();
    const tab: TableSchemaTab = {
      id,
      pinned: false,
      title: `${table} · Σ`,
      instanceId,
      kind: "table_schema",
      schema,
      table,
      data: emptySchemaState(),
    };
    set((s) => ({
      tabs: [...s.tabs, tab],
      activeTabId: activate ? id : s.activeTabId ?? id,
    }));
    get().logAction("tab.open", `table_schema ${schema}.${table}`);
  },

  openSqlTab: (instanceId, activate) => {
    const n = get().sqlTabCounter + 1;
    const id = newId();
    const tab: SqlWorkspaceTab = {
      id,
      pinned: false,
      title: `query #${n}`,
      instanceId,
      kind: "sql",
      data: emptySqlState("SELECT 1"),
    };
    set((s) => ({
      tabs: [...s.tabs, tab],
      sqlTabCounter: n,
      activeTabId: activate ? id : s.activeTabId ?? id,
    }));
    get().logAction("tab.open", `sql #${n}`);
  },

  openAdminTab: (instanceId, activate) => {
    const exist = get().tabs.find((t) => t.kind === "admin" && t.instanceId === instanceId);
    if (exist) {
      if (activate) set({ activeTabId: exist.id });
      return;
    }
    const id = newId();
    const tab: AdminWorkspaceTab = {
      id,
      pinned: false,
      title: "Admin",
      instanceId,
      kind: "admin",
      data: {},
    };
    set((s) => ({
      tabs: [...s.tabs, tab],
      activeTabId: activate ? id : s.activeTabId ?? id,
    }));
  },

  updateTableData: (tabId, patch) =>
    set((st) => ({
      tabs: st.tabs.map((t) =>
        t.id === tabId && t.kind === "table_data" ? { ...t, data: { ...t.data, ...patch } } : t,
      ) as WorkspaceTab[],
    })),

  updateSchemaTab: (tabId, patch) =>
    set((st) => ({
      tabs: st.tabs.map((t) =>
        t.id === tabId && t.kind === "table_schema" ? { ...t, data: { ...t.data, ...patch } } : t,
      ) as WorkspaceTab[],
    })),

  updateSqlTab: (tabId, patch) =>
    set((st) => ({
      tabs: st.tabs.map((t) =>
        t.id === tabId && t.kind === "sql" ? { ...t, data: { ...t.data, ...patch } } : t,
      ) as WorkspaceTab[],
    })),
}));
