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

/// Response for `POST /api/v1/projects/allocate` — new deploy slot on the server.
#[derive(Debug, Clone, Serialize)]
pub struct AllocateProjectResponse {
    pub id: String,
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

/// Per-project `app.env` next to `releases/` (sourced by `run.sh` or tooling).
#[derive(Debug, Clone, Serialize)]
pub struct AppEnvView {
    pub path: String,
    pub content: String,
    pub exists: bool,
}

/// Host server-stack environment file (typically `/etc/pirate-deploy.env`), editable from dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct HostDeployEnvView {
    pub path: String,
    pub content: String,
    pub exists: bool,
}

/// Result of writing host deploy env (e.g. after `sudo pirate-write-deploy-env.sh`).
#[derive(Debug, Clone, Serialize)]
pub struct HostDeployEnvPutView {
    pub path: String,
    pub content: String,
    pub exists: bool,
    /// Systemd delayed restarts were scheduled (deploy-server, then control-api).
    #[serde(default)]
    pub restart_scheduled: bool,
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

/// Состояние nginx на хосте (Ubuntu/systemd; для других ОС часть полей может быть пустой).
#[derive(Debug, Clone, Serialize)]
pub struct NginxStatusView {
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `active` | `inactive` | `failed` | `unknown` (нет systemctl и т.п.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub systemd_active: Option<String>,
    pub site_config_path: String,
    pub site_file_exists: bool,
    pub site_enabled: bool,
    pub ensure_script_present: bool,
    pub apply_site_script_present: bool,
}

/// One inventory row for `GET /api/v1/host-services`.
#[derive(Debug, Clone, Serialize)]
pub struct HostServiceRow {
    pub id: String,
    pub display_name: String,
    /// `runtime` | `web` | `database` | `storage`
    pub category: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub systemd_unit: Option<String>,
    /// `install` | `remove` | `none`
    pub actions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostServicesView {
    pub services: Vec<HostServiceRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cifs_mounts: Vec<String>,
    pub dispatch_script_present: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostServiceActionView {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxEnsureView {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_update: Option<NginxEnvUpdateView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxEnvVarUpdateView {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxEnvUpdateView {
    pub mode: String,
    pub restart_scheduled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<NginxEnvVarUpdateView>,
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

#[derive(Debug, Clone, Serialize)]
pub struct ProjectTelemetryLogLine {
    pub ts_ms: i64,
    pub level: String,
    pub message: String,
}

/// Snippet under `releases/<ver>/pirate-nginx-snippet.conf` when deploy wrote it.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectNginxSnippetView {
    pub path: String,
    pub configured: bool,
    /// `present` | `absent` | `no_release`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Machine-readable reason when `configured` is false (e.g. `not_nginx_edge`, `no_upstream_routes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// Short English explanation for operators; UI may localize by `reason_code`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectTelemetryView {
    pub project_id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_used_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_percent: Option<f32>,
    pub telemetry_available: bool,
    pub logs_available: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs_tail: Vec<ProjectTelemetryLogLine>,
    pub collected_at_ms: i64,
    pub project_nginx: ProjectNginxSnippetView,
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

// --- Anti-DDoS (host JSON + apply script) ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AntiddosFail2banConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_f2b_bantime")]
    pub bantime_sec: u32,
    #[serde(default = "default_f2b_findtime")]
    pub findtime_sec: u32,
    #[serde(default = "default_f2b_maxretry")]
    pub maxretry: u32,
}

fn default_true() -> bool {
    true
}
fn default_f2b_bantime() -> u32 {
    600
}
fn default_f2b_findtime() -> u32 {
    120
}
fn default_f2b_maxretry() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntiddosFirewallConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub syn_tuning: bool,
}

impl Default for AntiddosFirewallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            syn_tuning: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AntiddosLockdownConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tcp_ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntiddosHostConfig {
    #[serde(default = "default_schema_v1")]
    pub schema_version: u32,
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default = "default_rps")]
    pub rate_limit_rps: f64,
    #[serde(default = "default_burst")]
    pub burst: u32,
    #[serde(default = "default_mconn")]
    pub max_connections_per_ip: u32,
    #[serde(default = "default_cbt")]
    pub client_body_timeout_sec: u32,
    #[serde(default = "default_kat")]
    pub keepalive_timeout_sec: u32,
    #[serde(default = "default_snt")]
    pub send_timeout_sec: u32,
    #[serde(default)]
    pub whitelist_cidrs: Vec<String>,
    #[serde(default)]
    pub fail2ban: AntiddosFail2banConfig,
    #[serde(default)]
    pub firewall: AntiddosFirewallConfig,
    #[serde(default)]
    pub lockdown_app_ports: AntiddosLockdownConfig,
}

fn default_schema_v1() -> u32 {
    1
}
fn default_engine() -> String {
    "nginx_nft_fail2ban".to_string()
}
fn default_rps() -> f64 {
    10.0
}
fn default_burst() -> u32 {
    20
}
fn default_mconn() -> u32 {
    30
}
fn default_cbt() -> u32 {
    12
}
fn default_kat() -> u32 {
    20
}
fn default_snt() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AntiddosProjectConfig {
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default = "default_rps")]
    pub rate_limit_rps: f64,
    #[serde(default = "default_burst")]
    pub burst: u32,
    #[serde(default = "default_mconn")]
    pub max_connections_per_ip: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntiddosApplyResultView {
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AntiddosStatsView {
    pub fail2ban_jail: Option<String>,
    pub fail2ban_banned: Option<u32>,
    pub limit_log_tail: Vec<String>,
    pub nft_table_present: bool,
}
