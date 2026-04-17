//! Dashboard control plane: aggregate gRPC status, filesystem releases, PostgreSQL history, nginx file ops.

mod antiddos;
mod host_stats;
mod host_stats_detail;
mod host_stats_history;
mod host_services;
mod nginx;
mod service;
mod types;

pub use antiddos::{
    apply_antiddos_via_sudo, collect_antiddos_stats, default_host_config, read_host_json,
    validate_host_config, validate_project_config, write_host_json, write_project_json,
};
pub use host_stats::{collect_host_stats, NetCounters};
pub use host_stats_detail::{
    collect_cpu_detail, collect_disk_detail, collect_memory_detail, collect_network_detail,
    collect_processes_list,
};
pub use host_stats_history::HostStatsHistory;
pub use host_services::{
    collect_host_services, host_service_action_via_sudo, host_service_id_allowed, HOST_SERVICE_IDS,
};
pub use nginx::{
    apply_nginx_put, apply_nginx_site_via_sudo, collect_nginx_status, ensure_nginx_via_sudo,
    generate_nginx_server_config, nginx_route_conflicts, read_nginx_config, read_nginx_site_file,
    NginxPutOutcome,
};
pub use service::{ControlError, ControlPlane};
pub use types::{
    AllocateProjectResponse, AppEnvView, CpuDetail, CpuTimes, DataSourceItemView,
    DataSourcesListView, DatabaseColumnsView, DatabaseInfoView, DatabaseRelationshipsView,
    DatabaseSchemasView, DatabaseTablePreviewView, DatabaseTablesView, DiskDetail, DiskIoSummary,
    HistoryView, HostDeployEnvPutView, HostDeployEnvView, HostLogLine, HostMountStats,
    AntiddosHostConfig, AntiddosProjectConfig, AntiddosApplyResultView, AntiddosStatsView,
    HostServiceActionView, HostServiceRow, HostServicesView,
    HostNetInterface, HostStatsView, LocalClientConnect, LoadAvg, MemoryDetail, MemoryOverview,
    NetworkDetail, NginxConfigPut, NginxConfigView, NginxEnsureView, NginxEnvUpdateView,
    NginxEnvVarUpdateView, NginxPutResponseView, NginxStatusView, ProcessControlView,
    ProcessCpu, ProcessDisk, ProcessMem, ProcessRow, ProcessesDetail, ProjectNginxSnippetView,
    ProjectTelemetryLogLine, ProjectTelemetryView, ProjectView, ProjectsView, ReleasesView,
    RollbackBody, RollbackView,
    SeriesHint, SeriesPoint, SeriesResponse, SmbBrowseEntry, SmbBrowseView, StatusView,
};
