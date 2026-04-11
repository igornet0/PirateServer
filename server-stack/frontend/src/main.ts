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
import { loadNginx, saveNginx } from "./views/nginx.js";

initI18n();
onLocaleChange(() => {
  void refreshDashboard();
  void loadNginx();
  refreshDatabaseFromLocale();
});

function bootstrapDashboard(): void {
  if (!assertDashboardAccess()) {
    return;
  }

  const loading = t("loading");
  document.getElementById("status")!.textContent = loading;
  document.getElementById("releases")!.textContent = loading;
  document.getElementById("history")!.textContent = loading;
  (document.getElementById("projects") as HTMLElement).textContent = loading;

  applyPendingApiTokenFromLogin();

  initDashboardTabs();
  if (document.getElementById("tab-database")?.getAttribute("aria-selected") === "true") {
    void loadDatabaseInfo();
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
      void refreshDashboard();
    });
  }

  bindLifecycle();
  bindNginxWizard();
  bindHostServerDialog();
  bindDatabaseTab();

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

  void refreshDashboard();
  setInterval(() => {
    void refreshDashboard();
  }, 10_000);

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
