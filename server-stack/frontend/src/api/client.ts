import type {
  ApiErrorBody,
  HistoryView,
  NginxConfigView,
  NginxPutResponseView,
  ReleasesView,
  StatusView,
} from "./types.js";
import { ApiRequestError } from "./types.js";

function apiToken(): string {
  const el = document.getElementById("api-token") as HTMLInputElement | null;
  return el?.value?.trim() ?? "";
}

function nginxAdminToken(): string {
  const el = document.getElementById("nginx-token") as HTMLInputElement | null;
  return el?.value?.trim() ?? "";
}

function baseHeaders(): Record<string, string> {
  const h: Record<string, string> = {};
  const t = apiToken();
  if (t) {
    h.Authorization = `Bearer ${t}`;
  }
  return h;
}

async function parseError(res: Response): Promise<never> {
  const text = await res.text();
  try {
    const body = JSON.parse(text) as ApiErrorBody;
    if (body.error?.message) {
      throw new ApiRequestError(
        body.error.message,
        res.status,
        body.error.code,
      );
    }
  } catch (e) {
    if (e instanceof ApiRequestError) {
      throw e;
    }
    /* fall through */
  }
  throw new ApiRequestError(text || `HTTP ${res.status}`, res.status);
}

async function getJson<T>(path: string): Promise<T> {
  const r = await fetch(path, { headers: baseHeaders() });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<T>;
}

export async function fetchStatus(): Promise<StatusView> {
  return getJson<StatusView>("/api/v1/status");
}

export async function fetchReleases(): Promise<ReleasesView> {
  return getJson<ReleasesView>("/api/v1/releases");
}

export async function fetchHistory(): Promise<HistoryView> {
  return getJson<HistoryView>("/api/v1/history");
}

export async function fetchNginxConfig(): Promise<NginxConfigView> {
  return getJson<NginxConfigView>("/api/v1/nginx/config");
}

export async function putNginxConfig(content: string): Promise<NginxPutResponseView> {
  const api = apiToken();
  const nt = nginxAdminToken();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  if (api) {
    headers.Authorization = `Bearer ${api}`;
  }
  if (nt) {
    if (api) {
      headers["X-Nginx-Admin-Token"] = nt;
    } else {
      headers.Authorization = `Bearer ${nt}`;
    }
  }
  const r = await fetch("/api/v1/nginx/config", {
    method: "PUT",
    headers,
    body: JSON.stringify({ content }),
  });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<NginxPutResponseView>;
}
