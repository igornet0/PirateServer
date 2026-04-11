import {
  postProcessRestart,
  postProcessStop,
  postRollback,
} from "../api/client.js";
import { ApiRequestError } from "../api/types.js";
import { t } from "../i18n/index.js";
import { refreshDashboard } from "./dashboard.js";

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

export function bindLifecycle(): void {
  const resultEl = document.getElementById("lifecycle-result")!;

  document.getElementById("btn-rollback")?.addEventListener("click", async () => {
    const input = document.getElementById(
      "rollback-version",
    ) as HTMLInputElement | null;
    const version = input?.value?.trim() ?? "";
    if (!version) {
      resultEl.textContent = t("lifecycle.err.noVersion");
      return;
    }
    resultEl.textContent = t("lifecycle.rollingBack");
    try {
      const data = await postRollback(version);
      resultEl.textContent = JSON.stringify(data, null, 2);
      void refreshDashboard();
    } catch (e) {
      resultEl.textContent = formatErr(e);
    }
  });

  document.getElementById("btn-stop")?.addEventListener("click", async () => {
    resultEl.textContent = t("lifecycle.stopping");
    try {
      const data = await postProcessStop();
      resultEl.textContent = JSON.stringify(data, null, 2);
      void refreshDashboard();
    } catch (e) {
      resultEl.textContent = formatErr(e);
    }
  });

  document.getElementById("btn-restart")?.addEventListener("click", async () => {
    resultEl.textContent = t("lifecycle.restarting");
    try {
      const data = await postProcessRestart();
      resultEl.textContent = JSON.stringify(data, null, 2);
      void refreshDashboard();
    } catch (e) {
      resultEl.textContent = formatErr(e);
    }
  });
}
