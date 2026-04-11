import type {
  ApiErrorBody,
  HistoryView,
  NginxConfigView,
  NginxPutResponseView,
  ProcessControlView,
  ProjectsView,
  ReleasesView,
  RollbackView,
  StatusView,
} from "./types.js";
import { ApiRequestError } from "./types.js";

const ACCESS_TOKEN_KEY = "deploy.accessToken";

/**
 * Manual `#api-token` overrides session JWT; else JWT from login session.
 */
function apiToken(): string {
  const dash = document.getElementById("api-token") as HTMLInputElement | null;
  const manual = dash?.value?.trim() ?? "";
  if (manual) {
    return manual;
  }
  try {
    const s = sessionStorage.getItem(ACCESS_TOKEN_KEY);
    if (s?.trim()) {
      return s.trim();
    }
  } catch {
    /* ignore */
  }
  return "";
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

/** From dashboard `#active-project` input; persisted in sessionStorage. */
export function activeProject(): string {
  const el = document.getElementById("active-project") as HTMLInputElement | null;
  const v = el?.value?.trim();
  const p = v && v.length > 0 ? v : "default";
  try {
    sessionStorage.setItem("deploy.activeProject", p);
  } catch {
    /* ignore */
  }
  return p;
}

function projectQuery(): string {
  const p = encodeURIComponent(activeProject());
  return `?project=${p}`;
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

async function postJson<T>(path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...baseHeaders(),
  };
  const r = await fetch(path, {
    method: "POST",
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<T>;
}

export async function fetchStatus(): Promise<StatusView> {
  return getJson<StatusView>(`/api/v1/status${projectQuery()}`);
}

export async function fetchReleases(): Promise<ReleasesView> {
  return getJson<ReleasesView>(`/api/v1/releases${projectQuery()}`);
}

export async function fetchProjects(): Promise<ProjectsView> {
  return getJson<ProjectsView>("/api/v1/projects");
}

export async function fetchHistory(): Promise<HistoryView> {
  return getJson<HistoryView>(`/api/v1/history${projectQuery()}`);
}

export async function postRollback(version: string): Promise<RollbackView> {
  return postJson<RollbackView>("/api/v1/rollback", {
    version,
    project_id: activeProject(),
  });
}

export async function postProcessStop(): Promise<ProcessControlView> {
  return postJson<ProcessControlView>(
    `/api/v1/process/stop${projectQuery()}`,
  );
}

export async function postProcessRestart(): Promise<ProcessControlView> {
  return postJson<ProcessControlView>(
    `/api/v1/process/restart${projectQuery()}`,
  );
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
