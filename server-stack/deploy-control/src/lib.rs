//! Dashboard control plane: aggregate gRPC status, filesystem releases, PostgreSQL history, nginx file ops.

mod host_stats;
mod host_stats_detail;
mod host_stats_history;
mod nginx;
mod service;
mod types;

pub use host_stats::{collect_host_stats, NetCounters};
pub use host_stats_detail::{
    collect_cpu_detail, collect_disk_detail, collect_memory_detail, collect_network_detail,
    collect_processes_list,
};
pub use host_stats_history::HostStatsHistory;
pub use nginx::{apply_nginx_put, read_nginx_config, NginxPutOutcome};
pub use service::{ControlError, ControlPlane};
pub use types::{
    CpuDetail, CpuTimes, DataSourceItemView, DataSourcesListView, DatabaseColumnsView,
    DatabaseInfoView, DatabaseRelationshipsView, DatabaseSchemasView, DatabaseTablePreviewView,
    DatabaseTablesView, DiskDetail, DiskIoSummary, HistoryView, HostLogLine, HostMountStats,
    HostNetInterface, HostStatsView, LocalClientConnect, LoadAvg, MemoryDetail, MemoryOverview,
    NetworkDetail, NginxConfigPut, NginxConfigView, NginxPutResponseView, ProcessControlView,
    ProcessCpu, ProcessDisk, ProcessMem, ProcessRow, ProcessesDetail, ProjectView, ProjectsView,
    ReleasesView, RollbackBody, RollbackView, SeriesHint, SeriesPoint, SeriesResponse,
    SmbBrowseEntry, SmbBrowseView, StatusView,
};
