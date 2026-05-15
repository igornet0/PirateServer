//! Dashboard control plane: aggregate gRPC status, filesystem releases, PostgreSQL history, nginx file ops.

mod db_host;
mod db_admin;
mod db_migration;
mod acme_issue;
mod antiddos;
mod grpc_artifact_upload;
mod host_stats;
mod host_stats_detail;
mod host_stats_history;
mod host_services;
mod nginx;
mod nginx_probe;
mod nginx_universal;
mod pirate_storage;
mod storage_bind;
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
    collect_host_service_runtime_config, collect_host_services, filter_host_service_install_env,
    format_host_service_env_file, host_service_action_via_sudo, host_service_apply_runtime_via_sudo,
    host_service_id_allowed, host_service_restart_runtime_via_sudo, host_service_show_runtime_via_sudo,
    parse_host_service_env_file, HOST_SERVICE_IDS,
};
pub use nginx::{
    apply_nginx_inventory_file_put, apply_nginx_put, apply_nginx_site_via_sudo,
    apply_project_nginx_vhost, collect_nginx_status, ensure_nginx_via_sudo,
    generate_nginx_server_config, nginx_route_conflicts, parse_nginx_inventory_path,
    project_nginx_site_path, read_nginx_config, read_nginx_inventory_file, read_nginx_site_file,
    NginxPutOutcome,
};
pub use nginx_probe::{
    https_probe_failure_warrants_rollback, https_probe_localhost_resolve,
    https_probe_localhost_resolve_with_retries, openssl_x509_checkhost_pem,
};
pub use nginx_universal::{
    apply_nginx_universal_action, collect_nginx_sites, preflight_nginx, write_nginx_path_via_sudo_tee,
};
pub use service::{ControlError, ControlPlane};
pub use pirate_storage::{
    commit_uploaded_temp_file, create_folder, extract_archive, is_supported_archive_name, list_tree,
    normalize_rel_path, remove_dir, remove_file, rename_path, resolve_existing_path, resolve_path,
    storage_usage, storage_usage_no_db, store_uploaded_file, walk_storage_used_bytes,
    PirateStorageConfig,
    PirateStorageError, StorageEntryView, StorageExtractConflictMode, StorageExtractView,
    StorageListView, StorageUsageView,
};
pub use storage_bind::{
    default_storage_bind_state_path, list_storage_active_binds, list_storage_bind_mount_candidates,
    parse_bind_source_prefixes, storage_bind_sources_view, storage_bind_via_sudo,
    storage_bind_volume_name_ok, storage_unbind_via_sudo, StorageBindActive, StorageBindError,
    StorageBindMountCandidate, StorageBindSourcesView,
};
pub use db_migration::{host_migration_status, HostDbMigrationStatusView, MigrationToolReport};
pub use types::{
    AllocateProjectResponse, AppEnvView, CpuDetail, CpuTimes, DataSourceItemView,
    DataSourcesListView, DatabaseColumnsView, DatabaseInfoView, DatabaseRelationshipsView,
    DatabaseSchemasView, DatabaseTablePreviewView, DatabaseTablesView, DiskDetail, DiskIoSummary,
    HostDatabaseCapabilities, HostDatabaseInstanceView, HostDatabaseQueryBody, HostDatabaseQueryResultView,
    HostDatabaseRedisKeyView, HostDatabaseRedisKeysView, HostDatabasesListView, HostDbRequestCredentials,
    HostDbAdminCreateTableBody, HostDbAdminCreateTableColumn, HostDbAdminCreateUserBody, HostDbAdminCreateUserView,
    HostDbAdminDeleteUserBody, HostDbAdminDeleteUserView,
    HostDbCreateDatabaseBody, HostDbMigrationRunBody, HostDbMigrationRunView,
    HostDbV2CapabilitiesView, HostDbV2GridBody, HostDbV2GridView, HostDbV2MigrationStatusBody, HostDbV2MutationResultView,
    HostDbV2ObjectTreeView, HostDbV2RowMutationBody, HostDbV2SqlJobStartBody, HostDbV2SqlJobView,
    HistoryView, HostDeployEnvPutView, HostDeployEnvView, HostLogLine, HostMountStats,
    AntiddosHostConfig, AntiddosProjectConfig, AntiddosApplyResultView, AntiddosStatsView,
    HostServiceActionView, HostServiceRow, HostServiceRuntimeConfigView, HostServicesView,
    HostNetInterface, HostStatsView, LocalClientConnect, LoadAvg, MemoryDetail, MemoryOverview,
    NetworkDetail,     NginxActionBody, NginxActionPostCheckView, NginxActionResponseView, NginxConfigPut, NginxConfigView, NginxEnsureView,
    NginxEnvUpdateView, NginxEnvVarUpdateView, NginxPreflightProposed, NginxPreflightView,
    NginxFilePut, NginxProblemView, NginxPutResponseView, NginxSiteEntryView, NginxSitesView,
    NginxStatusView, ProjectNginxApplyBody, ProjectNginxApplyView,
    ProcessControlView,
    ProcessCpu, ProcessDisk, ProcessMem, ProcessRow, ProcessesDetail, ProjectNginxSnippetView,
    ProjectTelemetryLogLine, ProjectTelemetryView, ProjectView, ProjectsView, ReleasesView,
    RollbackBody, RollbackView,
    SeriesHint, SeriesPoint, SeriesResponse, SmbBrowseEntry, SmbBrowseView, StatusView,
};
