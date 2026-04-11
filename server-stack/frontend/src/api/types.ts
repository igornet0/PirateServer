/** Mirrors `deploy_control` / control-api JSON shapes. */

export interface StatusView {
  current_version: string;
  state: string;
  source: string;
}

export interface ReleasesView {
  releases: string[];
}

export interface ProjectView {
  id: string;
  deploy_root: string;
}

export interface ProjectsView {
  projects: ProjectView[];
}

export interface DeployEventRow {
  id: number;
  kind: string;
  version: string;
  created_at: string;
  state_snapshot: string | null;
  project_id: string;
}

export interface HistoryView {
  events: DeployEventRow[];
}

export interface NginxConfigView {
  path: string;
  content: string;
  enabled: boolean;
}

export interface NginxPutResponseView {
  ok: boolean;
  message: string;
  test_output?: string;
  reload_output?: string;
}

export interface RollbackView {
  status: string;
  active_version: string;
}

export interface ProcessControlView {
  current_version: string;
  state: string;
}

export interface ApiErrorPayload {
  code: string;
  message: string;
}

export interface ApiErrorBody {
  error: ApiErrorPayload;
}

export class ApiRequestError extends Error {
  readonly status: number;
  readonly code?: string;

  constructor(
    message: string,
    status: number,
    code?: string,
  ) {
    super(message);
    this.name = "ApiRequestError";
    this.status = status;
    this.code = code;
  }
}
