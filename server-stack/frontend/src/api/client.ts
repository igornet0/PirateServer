import { apiUrl } from "./api-base.js";
import { deployFetch } from "./deploy-fetch.js";
import type {
  ApiErrorBody,
  CpuDetail,
  DataSourceItemView,
  DataSourcesListView,
  DatabaseColumnsView,
  DatabaseInfoView,
  DatabaseRelationshipsView,
  DatabaseSchemasView,
  DatabaseTablePreviewView,
  DatabaseTablesView,
  DiskDetail,
  GrpcSessionsPageView,
  HistoryView,
  HostStatsDetailKind,
  HostStatsView,
  MemoryDetail,
  NetworkDetail,
  NginxConfigView,
  NginxPutResponseView,
  ProcessControlView,
  ProcessesDetail,
  ProjectsView,
  ReleasesView,
  RollbackView,
  RevokeProxySessionResponse,
  SeriesResponse,
  SmbBrowseView,
  StatusView,
  BootstrapHintsView,
  CreateProxySessionResponse,
  ProxySessionsPage,
} from "./types.js";
import { ApiRequestError } from "./types.js";

export { apiUrl };

const ACCESS_TOKEN_KEY = "deploy.accessToken";

/**
 * Manual `#api-token` overrides session JWT; else JWT from login session.
 */
export function apiToken(): string {
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
  const r = await deployFetch(apiUrl(path), { headers: baseHeaders() });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<T>;
}

/** Public gRPC URL from `DEPLOY_GRPC_PUBLIC_URL` (for Inbounds export). */
export async function fetchBootstrapHints(): Promise<BootstrapHintsView> {
  return getJson<BootstrapHintsView>("/api/v1/bootstrap-hints");
}

async function postJson<T>(path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...baseHeaders(),
  };
  const r = await deployFetch(apiUrl(path), {
    method: "POST",
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<T>;
}

async function patchJson<T>(path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...baseHeaders(),
  };
  const r = await deployFetch(apiUrl(path), {
    method: "PATCH",
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<T>;
}

async function deleteJson(path: string): Promise<void> {
  const r = await deployFetch(apiUrl(path), {
    method: "DELETE",
    headers: baseHeaders(),
  });
  if (!r.ok) {
    await parseError(r);
  }
}

export async function fetchStatus(): Promise<StatusView> {
  return getJson<StatusView>(`/api/v1/status${projectQuery()}`);
}

export async function fetchProxySessions(
  opts?: { limit?: number; offset?: number; revoked?: boolean },
): Promise<ProxySessionsPage> {
  const q = new URLSearchParams();
  q.set("project", activeProject());
  if (opts?.limit != null) {
    q.set("limit", String(opts.limit));
  }
  if (opts?.offset != null) {
    q.set("offset", String(opts.offset));
  }
  if (opts?.revoked != null) {
    q.set("revoked", String(opts.revoked));
  }
  return getJson<ProxySessionsPage>(`/api/v1/proxy-sessions?${q.toString()}`);
}

export async function createProxySession(body: {
  board_label: string;
  policy: Record<string, unknown>;
  recipient_client_pubkey_b64?: string;
  wire_mode?: number;
  wire_config?: Record<string, unknown>;
  ingress?: {
    protocol: number;
    listen_port: number;
    listen_udp_port?: number;
    config: Record<string, unknown>;
    tls?: Record<string, unknown>;
    template_version?: number;
  };
}): Promise<CreateProxySessionResponse> {
  const q = new URLSearchParams();
  q.set("project", activeProject());
  return postJson<CreateProxySessionResponse>(
    `/api/v1/proxy-sessions?${q.toString()}`,
    body,
  );
}

export async function patchProxySession(
  sessionId: string,
  body: {
    policy: Record<string, unknown>;
    wire_mode?: number;
    wire_config?: Record<string, unknown>;
    ingress?: {
      protocol: number;
      listen_port: number;
      listen_udp_port?: number;
      config: Record<string, unknown>;
      tls?: Record<string, unknown>;
      template_version?: number;
    };
    ingress_clear?: boolean;
  },
): Promise<{ status: string; expires_at_unix_ms: number }> {
  const q = new URLSearchParams();
  q.set("project", activeProject());
  return patchJson<{ status: string; expires_at_unix_ms: number }>(
    `/api/v1/proxy-sessions/${encodeURIComponent(sessionId)}?${q.toString()}`,
    body,
  );
}

export async function revokeProxySession(sessionId: string): Promise<RevokeProxySessionResponse> {
  return postJson<RevokeProxySessionResponse>(
    `/api/v1/proxy-sessions/${encodeURIComponent(sessionId)}/revoke`,
  );
}

/** Authenticated Xray JSON for a session (same document as the public subscription URL). */
export async function fetchProxySessionXrayConfig(sessionId: string): Promise<unknown> {
  const q = new URLSearchParams();
  q.set("project", activeProject());
  return getJson<unknown>(
    `/api/v1/proxy-sessions/${encodeURIComponent(sessionId)}/xray-config?${q.toString()}`,
  );
}

export async function fetchGrpcSessions(
  limit = 100,
  opts?: { includeTcpAudit?: boolean; onlineSecs?: number },
): Promise<GrpcSessionsPageView> {
  const q = new URLSearchParams();
  q.set("limit", String(Math.min(500, Math.max(1, limit))));
  if (opts?.includeTcpAudit) {
    q.set("include_tcp_audit", "true");
  }
  if (opts?.onlineSecs != null) {
    q.set("online_secs", String(opts.onlineSecs));
  }
  return getJson<GrpcSessionsPageView>(`/api/v1/grpc-sessions?${q.toString()}`);
}

export async function fetchHostStats(): Promise<HostStatsView> {
  return getJson<HostStatsView>("/api/v1/host-stats");
}

function hostStatsDetailPath(
  kind: HostStatsDetailKind,
  params?: Record<string, string | number | undefined>,
): string {
  let path = `/api/v1/host-stats/detail/${kind}`;
  const q = new URLSearchParams();
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== "") {
        q.set(k, String(v));
      }
    }
  }
  const qs = q.toString();
  return qs ? `${path}?${qs}` : path;
}

export async function fetchHostStatsDetail(
  kind: "cpu",
  params?: { top?: number },
): Promise<CpuDetail>;
export async function fetchHostStatsDetail(
  kind: "memory",
  params?: { top?: number },
): Promise<MemoryDetail>;
export async function fetchHostStatsDetail(
  kind: "disk",
  params?: { top?: number },
): Promise<DiskDetail>;
export async function fetchHostStatsDetail(
  kind: "network",
  params?: Record<string, never>,
): Promise<NetworkDetail>;
export async function fetchHostStatsDetail(
  kind: "processes",
  params?: { q?: string; limit?: number },
): Promise<ProcessesDetail>;
export async function fetchHostStatsDetail(
  kind: HostStatsDetailKind,
  params?: Record<string, string | number | undefined>,
): Promise<
  CpuDetail | MemoryDetail | DiskDetail | NetworkDetail | ProcessesDetail
> {
  return getJson(hostStatsDetailPath(kind, params));
}

/** Must match `parse_range_ms` in deploy-control `host_stats_history.rs`. */
export function normalizeSeriesRange(range: string): "15m" | "1h" | "24h" | "7d" {
  const r = range
    .trim()
    .toLowerCase()
    .replace(/\s+/g, "");
  if (r === "15m" || r === "15min") {
    return "15m";
  }
  if (r === "1h" || r === "60m" || r === "60min") {
    return "1h";
  }
  if (r === "24h" || r === "24hr" || r === "1d" || r === "1440m") {
    return "24h";
  }
  if (r === "7d" || r === "1w" || r === "week" || r === "168h" || r === "168hr") {
    return "7d";
  }
  return "1h";
}

export async function fetchHostStatsSeries(
  metric: string,
  range: string = "1h",
): Promise<SeriesResponse> {
  const q = new URLSearchParams({
    metric,
    range: normalizeSeriesRange(range),
  });
  return getJson<SeriesResponse>(`/api/v1/host-stats/series?${q}`);
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

export async function fetchDatabaseInfo(): Promise<DatabaseInfoView> {
  return getJson<DatabaseInfoView>("/api/v1/database-info");
}

export async function fetchDatabaseSchemas(): Promise<DatabaseSchemasView> {
  return getJson<DatabaseSchemasView>("/api/v1/database/schemas");
}

export async function fetchDatabaseTables(schema: string): Promise<DatabaseTablesView> {
  const q = new URLSearchParams({ schema });
  return getJson<DatabaseTablesView>(`/api/v1/database/tables?${q}`);
}

export async function fetchDatabaseColumns(
  schema: string,
  table: string,
): Promise<DatabaseColumnsView> {
  const path = `/api/v1/database/tables/${encodeURIComponent(schema)}/${encodeURIComponent(table)}/columns`;
  return getJson<DatabaseColumnsView>(path);
}

export async function fetchDatabaseTableRows(
  schema: string,
  table: string,
  opts?: { limit?: number; offset?: number },
): Promise<DatabaseTablePreviewView> {
  const q = new URLSearchParams();
  if (opts?.limit != null) {
    q.set("limit", String(opts.limit));
  }
  if (opts?.offset != null) {
    q.set("offset", String(opts.offset));
  }
  const qs = q.toString();
  const path = `/api/v1/database/tables/${encodeURIComponent(schema)}/${encodeURIComponent(table)}/rows${qs ? `?${qs}` : ""}`;
  return getJson<DatabaseTablePreviewView>(path);
}

export async function fetchDatabaseRelationships(): Promise<DatabaseRelationshipsView> {
  return getJson<DatabaseRelationshipsView>("/api/v1/database/relationships");
}

export async function fetchDataSources(): Promise<DataSourcesListView> {
  return getJson<DataSourcesListView>("/api/v1/data-sources");
}

export async function postDataSourceSmb(body: {
  label: string;
  host: string;
  share: string;
  folder: string;
  username: string;
  password: string;
}): Promise<DataSourceItemView> {
  return postJson<DataSourceItemView>("/api/v1/data-sources/smb", body);
}

export async function postDataSourceConnection(body: {
  kind: string;
  label: string;
  host: string;
  port: number;
  database?: string;
  username?: string;
  password?: string;
  ssl: boolean;
}): Promise<DataSourceItemView> {
  return postJson<DataSourceItemView>("/api/v1/data-sources/connection", body);
}

export async function deleteDataSource(id: string): Promise<void> {
  const path = `/api/v1/data-sources/${encodeURIComponent(id)}`;
  return deleteJson(path);
}

export async function fetchSmbBrowse(
  sourceId: string,
  path: string = "",
): Promise<SmbBrowseView> {
  const q = new URLSearchParams();
  if (path) {
    q.set("path", path);
  }
  const qs = q.toString();
  return getJson<SmbBrowseView>(
    `/api/v1/data-sources/${encodeURIComponent(sourceId)}/browse${qs ? `?${qs}` : ""}`,
  );
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
  const r = await deployFetch(apiUrl("/api/v1/nginx/config"), {
    method: "PUT",
    headers,
    body: JSON.stringify({ content }),
  });
  if (!r.ok) {
    await parseError(r);
  }
  return r.json() as Promise<NginxPutResponseView>;
}
