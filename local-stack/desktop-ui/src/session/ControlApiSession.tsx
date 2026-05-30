import React, { createContext, useCallback, useContext, useMemo, useRef, useState } from "react";
import { invoke as tauriInvoke, isTauri } from "@tauri-apps/api/core";

type ControlApiSessionValue = {
  controlApiBase: string;
  setControlApiBase: (url: string) => Promise<void>;
  /** Ensures Rust-side JWT routing uses the given base (defaults to context value). */
  ensureControlApiBase: (override?: string) => Promise<void>;
  sessionActive: boolean;
  refreshSessionActive: () => Promise<void>;
};

const ControlApiSessionContext = createContext<ControlApiSessionValue | null>(null);

export function ControlApiSessionProvider({
  initialBase,
  syncBase,
  children,
}: {
  initialBase: string;
  /** When set, keeps Rust + context aligned with the dashboard control-api field. */
  syncBase?: string;
  children: React.ReactNode;
}) {
  const [controlApiBase, setControlApiBaseState] = useState(initialBase);
  const [sessionActive, setSessionActive] = useState(false);
  const lastSyncedBase = useRef<string | null>(null);

  const setControlApiBase = useCallback(async (url: string) => {
    const trimmed = url.trim();
    setControlApiBaseState(trimmed);
    if (!isTauri()) return;
    await tauriInvoke("set_control_api_base", { url: trimmed });
    lastSyncedBase.current = trimmed;
  }, []);

  const ensureControlApiBase = useCallback(
    async (override?: string) => {
      if (!isTauri()) return;
      const trimmed = (override ?? controlApiBase).trim();
      if (lastSyncedBase.current === trimmed) return;
      await tauriInvoke("set_control_api_base", { url: trimmed });
      lastSyncedBase.current = trimmed;
    },
    [controlApiBase],
  );

  const refreshSessionActive = useCallback(async () => {
    if (!isTauri()) {
      setSessionActive(false);
      return;
    }
    const active = await tauriInvoke<boolean>("control_api_session_active");
    setSessionActive(active);
  }, []);

  React.useEffect(() => {
    if (syncBase === undefined) return;
    void setControlApiBase(syncBase);
  }, [syncBase, setControlApiBase]);

  const value = useMemo(
    () => ({
      controlApiBase,
      setControlApiBase,
      ensureControlApiBase,
      sessionActive,
      refreshSessionActive,
    }),
    [controlApiBase, setControlApiBase, ensureControlApiBase, sessionActive, refreshSessionActive],
  );

  return (
    <ControlApiSessionContext.Provider value={value}>{children}</ControlApiSessionContext.Provider>
  );
}

export function useControlApiSession(): ControlApiSessionValue {
  const ctx = useContext(ControlApiSessionContext);
  if (!ctx) {
    throw new Error("useControlApiSession must be used within ControlApiSessionProvider");
  }
  return ctx;
}
