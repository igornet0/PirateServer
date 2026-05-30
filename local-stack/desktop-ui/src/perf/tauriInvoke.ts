/**
 * Dev-only Tauri invoke wrapper (enable: `localStorage.setItem("pirateDesktop.perf", "1")`).
 */
import { invoke as tauriInvoke, isTauri } from "@tauri-apps/api/core";

function perfEnabled(): boolean {
  try {
    return localStorage.getItem("pirateDesktop.perf") === "1";
  } catch {
    return false;
  }
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error("Tauri invoke is only available in the desktop shell");
  }
  const t0 = performance.now();
  try {
    const out = await tauriInvoke<T>(cmd, args);
    if (perfEnabled()) {
      const ms = Math.round(performance.now() - t0);
      const size =
        typeof out === "string"
          ? out.length
          : out != null
            ? JSON.stringify(out).length
            : 0;
      console.debug(`[ipc] ${cmd} ${ms}ms ~${size}B`);
    }
    return out;
  } catch (e) {
    if (perfEnabled()) {
      const ms = Math.round(performance.now() - t0);
      console.debug(`[ipc] ${cmd} ${ms}ms ERR`, e);
    }
    throw e;
  }
}
