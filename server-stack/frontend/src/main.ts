import "uplot/dist/uPlot.min.css";
import { activeProject } from "./api/client.js";
import { initI18n, onLocaleChange, t } from "./i18n/index.js";
import { initDashboardTabs } from "./tabs.js";
import { refreshDashboard } from "./views/dashboard.js";
import { bindLifecycle } from "./views/lifecycle.js";
import {
  applyPendingApiTokenFromLogin,
  assertDashboardAccess,
  clearSessionAndReload,
} from "./views/login.js";
import { bindHostServerDialog } from "./views/host-server-dialog.js";
import { bindNginxWizard } from "./views/nginx-wizard.js";
import {
  bindDatabaseTab,
  loadDatabaseInfo,
  refreshDatabaseFromLocale,
} from "./views/database.js";
import { bindInboundsTab, loadInbounds } from "./views/inbounds.js";
import { loadNginx, saveNginx } from "./views/nginx.js";

let dashboardRefreshTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleDashboardRefresh(): void {
  if (dashboardRefreshTimer != null) {
    clearTimeout(dashboardRefreshTimer);
  }
  dashboardRefreshTimer = setTimeout(() => {
    dashboardRefreshTimer = null;
    void refreshDashboard();
  }, 200);
}

initI18n();
onLocaleChange(() => {
  scheduleDashboardRefresh();
  void loadNginx();
  refreshDatabaseFromLocale();
  void loadInbounds();
});

function bootstrapDashboard(): void {
  if (!assertDashboardAccess()) {
    return;
  }

  const loading = t("loading");
  const statusJsonEl = document.getElementById("status-json");
  if (statusJsonEl) {
    statusJsonEl.textContent = loading;
  }
  document.getElementById("status-kpi-strip")?.setAttribute("data-loading", "1");
  document.getElementById("releases")!.textContent = loading;
  document.getElementById("history")!.textContent = loading;
  (document.getElementById("projects") as HTMLElement).textContent = loading;

  applyPendingApiTokenFromLogin();

  initDashboardTabs();
  if (document.getElementById("tab-database")?.getAttribute("aria-selected") === "true") {
    void loadDatabaseInfo();
  }
  if (document.getElementById("tab-inbounds")?.getAttribute("aria-selected") === "true") {
    void loadInbounds();
  }
  const ap = document.getElementById("active-project") as HTMLInputElement | null;
  if (ap) {
    try {
      const s = sessionStorage.getItem("deploy.activeProject");
      if (s) {
        ap.value = s;
      }
    } catch {
      /* ignore */
    }
    ap.addEventListener("change", () => {
      void activeProject();
      scheduleDashboardRefresh();
    });
  }

  bindLifecycle();
  bindNginxWizard();
  bindHostServerDialog();
  bindDatabaseTab();
  bindInboundsTab();

  document.getElementById("btn-copy-local-client")?.addEventListener("click", async () => {
    const text = document.getElementById("local-client-json")?.textContent?.trim();
    if (!text) {
      return;
    }
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* ignore — clipboard may be denied */
    }
  });

  document.getElementById("btn-copy-display-stream")?.addEventListener("click", async () => {
    const text = document.getElementById("display-stream-data-url")?.textContent?.trim();
    if (!text) {
      return;
    }
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* ignore */
    }
  });

  scheduleDashboardRefresh();
  setInterval(scheduleDashboardRefresh, 10_000);

  document.getElementById("nginx-load")?.addEventListener("click", () => {
    void loadNginx();
  });
  document.getElementById("nginx-save")?.addEventListener("click", () => {
    void saveNginx();
  });

  document.getElementById("btn-logout")?.addEventListener("click", () => {
    clearSessionAndReload();
  });

  void loadNginx();
}

bootstrapDashboard();
