use deploy_db::DeployEventRow;
use deploy_db::{ForeignKeyRow, SchemaRow, TableColumnRow, TablePreview, TableSummaryRow};
use serde::{Deserialize, Serialize};

/// Deploy status exposed to the HTTP API (and dashboard).
/// One deployable unit on the host. Today only a single implicit `default` project (see ROADMAP multi-project).
#[derive(Debug, Clone, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub deploy_root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectsView {
    pub projects: Vec<ProjectView>,
}

/// Install-style JSON for local clients. With gRPC auth: `token`, `url`, `pairing`.
/// With `DEPLOY_GRPC_ALLOW_UNAUTHENTICATED`, only `url` is set (empty fields omitted in JSON).
#[derive(Debug, Clone, Serialize)]
pub struct LocalClientConnect {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pairing: String,
}

/// One filesystem mount (sysinfo disks list).
#[derive(Debug, Clone, Serialize)]
pub struct HostMountStats {
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Per-interface traffic snapshot (rates require previous counters in control-api).
#[derive(Debug, Clone, Serialize)]
pub struct HostNetInterface {
    pub name: String,
    pub rx_bytes_per_s: f64,
    pub tx_bytes_per_s: f64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

/// Tail line from an application log file (when `CONTROL_API_LOG_TAIL_PATH` is set).
#[derive(Debug, Clone, Serialize)]
pub struct HostLogLine {
    pub ts_ms: i64,
    pub level: String,
    pub message: String,
}

/// Host resource snapshot for the machine running control-api (same host as `deploy_root` disk).
#[derive(Debug, Clone, Serialize)]
pub struct HostStatsView {
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    /// Mount point used for disk figures (longest prefix of `deploy_root`).
    pub disk_mount_path: String,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    /// Instantaneous global CPU usage (%), sampled over a short interval.
    pub cpu_usage_percent: f32,
    /// Unix load averages (not CPU %); unavailable platforms may be 0.
    pub load_average_1m: f64,
    pub load_average_5m: f64,
    pub load_average_15m: f64,
    /// Highest sensor reading (°C), if any hardware exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_current_celsius: Option<f32>,
    /// Mean of valid sensor readings (°C).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_avg_celsius: Option<f32>,
    pub process_count: usize,
    /// All mounts reported by the OS (may include non-deploy volumes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disk_mounts: Vec<HostMountStats>,
    /// Network interfaces with throughput estimates (non-zero rates after a prior sample in-process).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_interfaces: Vec<HostNetInterface>,
    /// Last lines from the configured app log path (empty if unset or unreadable).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log_tail: Vec<HostLogLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusView {
    pub current_version: String,
    pub state: String,
    /// `grpc` when live gRPC succeeded; `database` when falling back to snapshot.
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_client: Option<LocalClientConnect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleasesView {
    pub releases: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryView {
    pub events: Vec<DeployEventRow>,
}

/// PostgreSQL metadata for the control-api connection (password never included).
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseInfoView {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_connections: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseSchemasView {
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<SchemaRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseTablesView {
    pub configured: bool,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<TableSummaryRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseColumnsView {
    pub configured: bool,
    pub schema: String,
    pub table: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<TableColumnRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseRelationshipsView {
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKeyRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseTablePreviewView {
    pub configured: bool,
    pub schema: String,
    pub table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<TablePreview>,
}

/// Dashboard data source (PostgreSQL is synthetic; other rows come from `data_sources`).
#[derive(Debug, Clone, Serialize)]
pub struct DataSourceItemView {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smb_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smb_share: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smb_subpath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_point: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Sanitized `config_json` (no passwords).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_json: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_credentials: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DataSourcesListView {
    pub sources: Vec<DataSourceItemView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SmbBrowseEntry {
    pub name: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SmbBrowseView {
    pub source_id: String,
    pub path: String,
    pub entries: Vec<SmbBrowseEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxConfigView {
    pub path: String,
    pub content: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct NginxConfigPut {
    pub content: String,
}

/// Result of gRPC `Rollback`.
#[derive(Debug, Clone, Serialize)]
pub struct RollbackView {
    pub status: String,
    pub active_version: String,
}

/// Result of `StopProcess` / `RestartProcess` (same shape as deploy status).
#[derive(Debug, Clone, Serialize)]
pub struct ProcessControlView {
    pub current_version: String,
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct RollbackBody {
    pub version: String,
    /// Empty or omitted means `default`.
    #[serde(default)]
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxPutResponseView {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reload_output: Option<String>,
}

// --- Host stats detail (on-demand) ---

#[derive(Debug, Clone, Serialize)]
pub struct LoadAvg {
    pub m1: f64,
    pub m5: f64,
    pub m15: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuDetail {
    pub ts_ms: i64,
    pub loadavg: LoadAvg,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub times: Option<CpuTimes>,
    pub top_processes: Vec<ProcessCpu>,
    pub series_hint: SeriesHint,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuTimes {
    pub user_ms: u64,
    pub system_ms: u64,
    pub idle_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessCpu {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesHint {
    pub available_ranges: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOverview {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffers_bytes: Option<u64>,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryDetail {
    pub ts_ms: i64,
    pub memory: MemoryOverview,
    pub top_processes: Vec<ProcessMem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessMem {
    pub pid: u32,
    pub name: String,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskIoSummary {
    pub note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskDetail {
    pub ts_ms: i64,
    pub mounts: Vec<HostMountStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io: Option<DiskIoSummary>,
    pub top_processes: Vec<ProcessDisk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessDisk {
    pub pid: u32,
    pub name: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkDetail {
    pub ts_ms: i64,
    pub interfaces: Vec<HostNetInterface>,
    pub connections_note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessesDetail {
    pub ts_ms: i64,
    pub processes: Vec<ProcessRow>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesResponse {
    pub metric: String,
    pub step_ms: u64,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesPoint {
    pub ts_ms: i64,
    pub value: f64,
}
