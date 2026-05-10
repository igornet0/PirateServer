import { create } from "zustand";

export type DirectProfile = {
  id: string;
  name: string;
  engine: string;
  host: string;
  port: number;
  databaseName: string | null;
  username: string | null;
  sslMode: string;
  groupTag: string | null;
  orderIndex: number;
  lastOkAtMs: number | null;
  hasSavedPassword: boolean;
};

export type QueryResult = {
  columns: string[];
  rowCount: number;
  rows: Record<string, unknown>[];
  truncated: boolean;
  warn: string | null;
};

export type CenterTab = "data" | "sql" | "stats" | "structure" | "tunnels";

type S = {
  profiles: DirectProfile[];
  activeProfileId: string | null;
  /** Profile the current `sessionId` was opened for (unchanged by list selection). */
  sessionProfileId: string | null;
  sessionId: string | null;
  connectBusy: boolean;
  lastError: string | null;
  schemas: string[];
  selectedSchema: string | null;
  tables: string[];
  selectedTable: string | null;
  preview: QueryResult | null;
  sqlText: string;
  sqlResult: QueryResult | null;
  sqlBusy: boolean;
  centerTab: CenterTab;
  historyJson: string | null;
  statsJson: string | null;
  structureJson: string | null;
};

type A = {
  setProfiles: (p: DirectProfile[]) => void;
  setSession: (id: string | null, profileId: string | null) => void;
  setConnectBusy: (b: boolean) => void;
  setLastError: (e: string | null) => void;
  setSchemas: (s: string[]) => void;
  setTables: (t: string[]) => void;
  setPreview: (p: QueryResult | null) => void;
  setSqlText: (s: string) => void;
  setSqlResult: (p: QueryResult | null) => void;
  setSqlBusy: (b: boolean) => void;
  setCenterTab: (t: CenterTab) => void;
  setSelectedSchema: (s: string | null) => void;
  setSelectedTable: (s: string | null) => void;
  setHistoryJson: (j: string | null) => void;
  setStatsJson: (j: string | null) => void;
  setStructureJson: (j: string | null) => void;
  resetAfterDisconnect: () => void;
  selectProfile: (id: string | null) => void;
};

export const useDbExplorerStore = create<S & A>((set) => ({
  profiles: [],
  activeProfileId: null,
  sessionProfileId: null,
  sessionId: null,
  connectBusy: false,
  lastError: null,
  schemas: [],
  selectedSchema: null,
  tables: [],
  selectedTable: null,
  preview: null,
  sqlText: "SELECT 1",
  sqlResult: null,
  sqlBusy: false,
  centerTab: "data",
  historyJson: null,
  statsJson: null,
  structureJson: null,
  setProfiles: (profiles) => set({ profiles }),
  setSession: (sessionId, activeProfileId) =>
    set({ sessionId, activeProfileId, sessionProfileId: activeProfileId }),
  setConnectBusy: (connectBusy) => set({ connectBusy }),
  setLastError: (lastError) => set({ lastError }),
  setSchemas: (schemas) => set({ schemas }),
  setTables: (tables) => set({ tables }),
  setPreview: (preview) => set({ preview }),
  setSqlText: (sqlText) => set({ sqlText }),
  setSqlResult: (sqlResult) => set({ sqlResult }),
  setSqlBusy: (sqlBusy) => set({ sqlBusy }),
  setCenterTab: (centerTab) => set({ centerTab }),
  setSelectedSchema: (selectedSchema) => set({ selectedSchema }),
  setSelectedTable: (selectedTable) => set({ selectedTable }),
  setHistoryJson: (historyJson) => set({ historyJson }),
  setStatsJson: (statsJson) => set({ statsJson }),
  setStructureJson: (structureJson) => set({ structureJson }),
  resetAfterDisconnect: () =>
    set({
      sessionId: null,
      sessionProfileId: null,
      activeProfileId: null,
      schemas: [],
      selectedSchema: null,
      tables: [],
      selectedTable: null,
      preview: null,
      sqlResult: null,
      statsJson: null,
      structureJson: null,
    }),
  selectProfile: (id) => set({ activeProfileId: id }),
}));
