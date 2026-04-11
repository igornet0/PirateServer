import {
  fetchHistory,
  fetchProjects,
  fetchReleases,
  fetchStatus,
} from "../api/client.js";
import { ApiRequestError } from "../api/types.js";
import { t } from "../i18n/index.js";

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

export async function refreshDashboard(): Promise<void> {
  const statusEl = document.getElementById("status")!;
  const projectsEl = document.getElementById("projects")! as HTMLElement;
  const releasesEl = document.getElementById("releases")!;
  const historyEl = document.getElementById("history")!;

  const bundleWrap = document.getElementById("local-client-bundle") as HTMLElement | null;
  const bundleJson = document.getElementById("local-client-json") as HTMLElement | null;

  try {
    const data = await fetchStatus();
    const { local_client: lc, ...core } = data;
    statusEl.textContent = JSON.stringify(core, null, 2);
    if (bundleWrap && bundleJson) {
      const bundle: Record<string, string> = {};
      if (lc?.token) {
        bundle.token = lc.token;
      }
      if (lc?.url) {
        bundle.url = lc.url;
      }
      if (lc?.pairing) {
        bundle.pairing = lc.pairing;
      }
      if (Object.keys(bundle).length > 0) {
        bundleWrap.hidden = false;
        bundleJson.textContent = JSON.stringify(bundle, null, 2);
      } else {
        bundleWrap.hidden = true;
        bundleJson.textContent = "";
      }
    }
  } catch (e) {
    statusEl.textContent = formatErr(e);
    if (bundleWrap && bundleJson) {
      bundleWrap.hidden = true;
      bundleJson.textContent = "";
    }
  }

  try {
    const data = await fetchProjects();
    const rows = data.projects
      .map(
        (p) =>
          `<tr><td><code>${escapeHtml(p.id)}</code></td><td>${escapeHtml(p.deploy_root)}</td></tr>`,
      )
      .join("");
    projectsEl.innerHTML =
      `<table class="proj-table"><thead><tr><th>${escapeHtml(t("inv.table.id"))}</th><th>${escapeHtml(t("inv.table.deployRoot"))}</th></tr></thead><tbody>${rows}</tbody></table>`;
  } catch (e) {
    projectsEl.textContent = formatErr(e);
  }

  try {
    const data = await fetchReleases();
    releasesEl.textContent = JSON.stringify(data, null, 2);
  } catch (e) {
    releasesEl.textContent = formatErr(e);
  }

  try {
    const data = await fetchHistory();
    historyEl.textContent = JSON.stringify(data, null, 2);
  } catch (e) {
    historyEl.textContent = formatErr(e);
  }
}
