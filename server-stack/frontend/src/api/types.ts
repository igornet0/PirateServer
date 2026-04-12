/** Mirrors `deploy_control` / control-api JSON shapes. */

/** Matches deploy-server install JSON: `token`, `url`, `pairing`. */
export interface LocalClientConnect {
  token: string;
  url: string;
  pairing: string;
}

export interface StatusView {
  current_version: string;
  state: string;
  source: string;
  local_client?: LocalClientConnect | null;
}

/** `GET /api/v1/grpc-sessions` — deploy-server gRPC audit (metadata DB). */
export interface GrpcSessionsSummaryView {
  total_events: number;
  tcp_open_total: number;
  tcp_close_total: number;
  estimated_open_tcp: number;
  closed_tcp_events: number;
  by_kind: Record<string, number>;
}

export interface GrpcSessionEventView {
  id: number;
  created_at: string;
  kind: string;
  peer_ip: string;
  status: string;
  grpc_method: string;
  client_public_key_b64?: string | null;
  detail: string;
}

/** One row per known client key — last activity from the metadata DB. */
export interface GrpcSessionPeerView {
  client_public_key_b64: string;
  last_seen_at: string;
  last_peer_ip: string;
  last_grpc_method: string;
  online: boolean;
  connection_kind: number;
  last_cpu_percent?: number | null;
  last_ram_percent?: number | null;
  last_gpu_percent?: number | null;
  proxy_bytes_in_total: number;
  proxy_bytes_out_total: number;
}

/** Latest `deploy-server resource-benchmark` row (this host). */
export interface ServerBenchmarkView {
  run_at: string;
  cpu_score: number;
  ram_score: number;
  storage_score: number;
  gpu_score?: number | null;
}

export interface GrpcSessionsPageView {
  summary: GrpcSessionsSummaryView;
  peers: GrpcSessionPeerView[];
  recent: GrpcSessionEventView[];
  server_benchmark?: ServerBenchmarkView | null;
}

/** `GET /api/v1/database-info` — optional PostgreSQL explorer (password never returned). */
export interface DatabaseInfoView {
  configured: boolean;
  connection_display?: string | null;
  server_version?: string | null;
  database_name?: string | null;
  session_user?: string | null;
  database_size_bytes?: number | null;
  active_connections?: number | null;
}

export interface SchemaRow {
  name: string;
}

export interface TableSummaryRow {
  schema_name: string;
  name: string;
  table_type: string;
  row_estimate: number | null;
}

export interface TableColumnRow {
  column_name: string;
  data_type: string;
  is_nullable: string;
  column_default: string | null;
  character_maximum_length: number | null;
  numeric_precision: number | null;
  numeric_scale: number | null;
}

export interface ForeignKeyRow {
  table_schema: string;
  table_name: string;
  column_name: string;
  foreign_table_schema: string;
  foreign_table_name: string;
  foreign_column_name: string;
  constraint_name: string;
}

export interface TablePreview {
  rows: unknown;
}

export interface DatabaseSchemasView {
  configured: boolean;
  schemas?: SchemaRow[];
}

export interface DatabaseTablesView {
  configured: boolean;
  schema: string;
  tables?: TableSummaryRow[];
}

export interface DatabaseColumnsView {
  configured: boolean;
  schema: string;
  table: string;
  columns?: TableColumnRow[];
}

export interface DatabaseRelationshipsView {
  configured: boolean;
  foreign_keys?: ForeignKeyRow[];
}

export interface DatabaseTablePreviewView {
  configured: boolean;
  schema: string;
  table: string;
  preview?: TablePreview | null;
}

/** `GET /api/v1/data-sources` — PostgreSQL is synthetic; other rows from DB (no passwords). */
export interface DataSourceItemView {
  id: string;
  kind: string;
  label: string;
  mount_state?: string | null;
  smb_host?: string | null;
  smb_share?: string | null;
  smb_subpath?: string | null;
  mount_point?: string | null;
  last_error?: string | null;
  /** Sanitized connection settings (no passwords). */
  config_json?: Record<string, unknown> | null;
  has_credentials?: boolean | null;
}

export interface DataSourcesListView {
  sources: DataSourceItemView[];
}

export interface SmbBrowseEntry {
  name: string;
  is_dir: boolean;
  size?: number | null;
}

export interface SmbBrowseView {
  source_id: string;
  path: string;
  entries: SmbBrowseEntry[];
}

/** JSON body of `GET /api/v1/host-stats` (control-api / deploy-control). */
export interface HostMountStats {
  path: string;
  total_bytes: number;
  free_bytes: number;
}

export interface HostNetInterface {
  name: string;
  rx_bytes_per_s: number;
  tx_bytes_per_s: number;
  rx_errors: number;
  tx_errors: number;
}

export interface HostLogLine {
  ts_ms: number;
  level: string;
  message: string;
}

export interface HostStatsView {
  disk_free_bytes: number;
  disk_total_bytes: number;
  disk_mount_path: string;
  memory_used_bytes: number;
  memory_total_bytes: number;
  cpu_usage_percent: number;
  load_average_1m: number;
  load_average_5m: number;
  load_average_15m: number;
  temperature_current_celsius?: number | null;
  temperature_avg_celsius?: number | null;
  process_count: number;
  disk_mounts?: HostMountStats[];
  network_interfaces?: HostNetInterface[];
  log_tail?: HostLogLine[];
}

export interface LoadAvg {
  m1: number;
  m5: number;
  m15: number;
}

export interface CpuTimes {
  user_ms: number;
  system_ms: number;
  idle_ms: number;
}

export interface ProcessCpu {
  pid: number;
  name: string;
  cpu_percent: number;
}

export interface SeriesHint {
  available_ranges: string[];
}

export interface CpuDetail {
  ts_ms: number;
  loadavg: LoadAvg;
  times?: CpuTimes | null;
  top_processes: ProcessCpu[];
  series_hint: SeriesHint;
}

export interface MemoryOverview {
  total_bytes: number;
  used_bytes: number;
  available_bytes: number;
  cached_bytes?: number | null;
  buffers_bytes?: number | null;
  swap_total_bytes: number;
  swap_used_bytes: number;
}

export interface ProcessMem {
  pid: number;
  name: string;
  memory_bytes: number;
}

export interface MemoryDetail {
  ts_ms: number;
  memory: MemoryOverview;
  top_processes: ProcessMem[];
}

export interface DiskIoSummary {
  note: string;
}

export interface ProcessDisk {
  pid: number;
  name: string;
  read_bytes: number;
  write_bytes: number;
}

export interface DiskDetail {
  ts_ms: number;
  mounts: HostMountStats[];
  io?: DiskIoSummary | null;
  top_processes: ProcessDisk[];
}

export interface NetworkDetail {
  ts_ms: number;
  interfaces: HostNetInterface[];
  connections_note: string;
}

export interface ProcessRow {
  pid: number;
  name: string;
  cpu_percent: number;
  memory_bytes: number;
}

export interface ProcessesDetail {
  ts_ms: number;
  processes: ProcessRow[];
  total: number;
}

export interface SeriesPoint {
  ts_ms: number;
  value: number;
}

export interface SeriesResponse {
  metric: string;
  step_ms: number;
  points: SeriesPoint[];
}

export type HostStatsDetailKind =
  | "cpu"
  | "memory"
  | "disk"
  | "network"
  | "processes";

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
