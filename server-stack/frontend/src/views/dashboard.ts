import {
  fetchHistory,
  fetchReleases,
  fetchStatus,
} from "../api/client.js";
import { ApiRequestError } from "../api/types.js";

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

export async function refreshDashboard(): Promise<void> {
  const statusEl = document.getElementById("status")!;
  const releasesEl = document.getElementById("releases")!;
  const historyEl = document.getElementById("history")!;

  try {
    const data = await fetchStatus();
    statusEl.textContent = JSON.stringify(data, null, 2);
  } catch (e) {
    statusEl.textContent = formatErr(e);
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
