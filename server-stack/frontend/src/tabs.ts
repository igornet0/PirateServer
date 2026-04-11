const STORAGE_KEY = "deploy.activeTab";

export function initDashboardTabs(): void {
  const tablist = document.querySelector<HTMLElement>('[data-tablist="dashboard"]');
  if (!tablist) return;

  const tabs = Array.from(
    tablist.querySelectorAll<HTMLButtonElement>('[role="tab"]'),
  );
  const panels = tabs.map((t) => {
    const id = t.getAttribute("aria-controls");
    const el = id ? document.getElementById(id) : null;
    return el as HTMLElement | null;
  });

  if (tabs.length === 0 || panels.some((p) => !p)) return;

  function activate(index: number): void {
    const i = ((index % tabs.length) + tabs.length) % tabs.length;
    tabs.forEach((tab, j) => {
      const selected = j === i;
      tab.setAttribute("aria-selected", selected ? "true" : "false");
      tab.tabIndex = selected ? 0 : -1;
    });
    panels.forEach((panel, j) => {
      if (!panel) return;
      panel.hidden = j !== i;
    });
    try {
      sessionStorage.setItem(STORAGE_KEY, tabs[i]!.id);
    } catch {
      /* ignore */
    }
  }

  function currentIndex(): number {
    const sel = tabs.findIndex((t) => t.getAttribute("aria-selected") === "true");
    return sel >= 0 ? sel : 0;
  }

  tabs.forEach((tab, i) => {
    tab.addEventListener("click", () => activate(i));
    tab.addEventListener("keydown", (ev) => {
      if (ev.key === "ArrowRight" || ev.key === "ArrowDown") {
        ev.preventDefault();
        const next = (currentIndex() + 1) % tabs.length;
        activate(next);
        tabs[next]?.focus();
      } else if (ev.key === "ArrowLeft" || ev.key === "ArrowUp") {
        ev.preventDefault();
        const next = (currentIndex() - 1 + tabs.length) % tabs.length;
        activate(next);
        tabs[next]?.focus();
      } else if (ev.key === "Home") {
        ev.preventDefault();
        activate(0);
        tabs[0]?.focus();
      } else if (ev.key === "End") {
        ev.preventDefault();
        activate(tabs.length - 1);
        tabs[tabs.length - 1]?.focus();
      }
    });
  });

  let initial = 0;
  try {
    const saved = sessionStorage.getItem(STORAGE_KEY);
    if (saved) {
      const idx = tabs.findIndex((t) => t.id === saved);
      if (idx >= 0) initial = idx;
    }
  } catch {
    /* ignore */
  }
  activate(initial);
}
