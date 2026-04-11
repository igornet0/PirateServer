//! JSON DTOs for local monitoring HTTP API.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct MonitoringOverview {
    pub ts_ms: i64,
    pub disk: DiskOverview,
    pub memory: MemoryOverview,
    pub cpu: CpuOverview,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<TemperatureOverview>,
    pub process_count: usize,
    pub network: NetworkOverview,
    pub logs: LogsOverview,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskOverview {
    pub mounts: Vec<MountStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MountStats {
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
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
pub struct CpuOverview {
    pub usage_percent: f32,
    pub loadavg: LoadAvg,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadAvg {
    pub m1: f64,
    pub m5: f64,
    pub m15: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemperatureOverview {
    pub current_max: f32,
    pub avg: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkOverview {
    pub interfaces: Vec<InterfaceTraffic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceTraffic {
    pub name: String,
    pub rx_bytes_per_s: f64,
    pub tx_bytes_per_s: f64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogsOverview {
    pub items: Vec<LogItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogItem {
    pub ts_ms: i64,
    pub level: String,
    pub message: String,
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
pub struct DiskDetail {
    pub ts_ms: i64,
    pub mounts: Vec<MountStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io: Option<DiskIoSummary>,
    pub top_processes: Vec<ProcessDisk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskIoSummary {
    pub note: &'static str,
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
    pub interfaces: Vec<InterfaceTraffic>,
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
pub struct LogsDetail {
    pub items: Vec<LogItem>,
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

#[derive(Debug, Clone, Serialize)]
pub struct ExportSnapshot {
    pub overview: MonitoringOverview,
    pub exported_ts_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlertsStatus {
    pub alerts_enabled: bool,
    pub economy_mode: bool,
    pub triggered: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct StreamSubscribe {
    #[serde(default)]
    pub op: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub interval_ms: Option<u64>,
}
