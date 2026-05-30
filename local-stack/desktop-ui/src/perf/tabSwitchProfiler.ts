/**
 * Dev tab-switch profiler (enable: `localStorage.setItem("pirateDesktop.perf", "1")`).
 */
let lastTab: string | null = null;
let lastSwitchAt = 0;

export function recordTabSwitch(tab: string): void {
  try {
    if (localStorage.getItem("pirateDesktop.perf") !== "1") return;
  } catch {
    return;
  }
  const now = performance.now();
  if (lastTab != null) {
    console.debug(`[tab] ${lastTab} → ${tab} ${Math.round(now - lastSwitchAt)}ms since last switch`);
  }
  lastTab = tab;
  lastSwitchAt = now;
}
