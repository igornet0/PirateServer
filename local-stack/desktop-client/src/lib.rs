//! Pirate Client desktop library (Tauri backend helpers).

pub mod bookmarks;
pub mod cmd_vars;
pub mod connection;
pub mod control_api;
mod http_client;
mod perf;
mod tokio_runtime;
mod control_api_keychain;
mod credentials_crypto;
mod db_credentials;
pub mod db_direct;
mod db_tunnel;
pub mod deploy;
mod desktop_store;
pub mod display_ingest;
mod display_stream_prefs;
pub mod env_editor;
mod env_paths;
pub mod host_agent;
mod host_services_compat;
pub mod host_stats;
pub mod hosts;
pub mod internet_proxy;
pub mod local_dev_stack;
pub mod monitoring;
pub mod paas;
mod manifest_fix;
mod preflight;
pub mod project_registry;
pub mod server_stack;
mod ssl_ops;
mod stack_tun_remote;
pub mod status;
pub mod toolchain_probe;
pub use env_editor::{read_project_local_env, write_project_local_env, LocalEnvView};

pub use bookmarks::{
    bookmark_by_id, load_bookmarks, remove_bookmark, set_bookmark_host_agent, set_bookmark_label,
    upsert_bookmark, ServerBookmark,
};
pub use connection::{
    add_bookmark_from_input, clear_endpoint, connect_from_bundle, control_api_recent_restart_hint,
    ensure_unified_client_config_migrated, load_control_api_base, load_control_api_direct_url,
    load_endpoint, load_project_id, mark_control_api_recent_restart, migrate_grpc_public_endpoint,
    parse_grpc_endpoint_from_bundle, save_endpoint, set_active_project, set_control_api_base,
    verify_grpc_endpoint, verify_grpc_status_for_project, verify_grpc_status_for_project_async,
    GrpcConnectResult,
};
pub use control_api::{
    allocate_and_apply_remote_project_id, allocate_remote_project_id, control_api_antiddos_apply,
    control_api_antiddos_disable, control_api_antiddos_enable, control_api_antiddos_get_json,
    control_api_antiddos_project_delete, control_api_antiddos_project_put_json,
    control_api_antiddos_put_json, control_api_antiddos_stats_json,
    control_api_apply_project_nginx, control_api_bearer_token,
    control_api_clear_project_runtime_log, control_api_ensure_nginx,
    control_api_fetch_app_env_json, control_api_fetch_host_deploy_env_json,
    control_api_fetch_host_deploy_env_template_json, control_api_fetch_host_services_json,
    control_api_fetch_nginx_file_json, control_api_fetch_nginx_site_json,
    control_api_fetch_nginx_sites_json, control_api_fetch_nginx_status_json,
    control_api_fetch_process_listeners_json, control_api_fetch_project_telemetry_json,
    control_api_fetch_status_json, control_api_health_probe, control_api_host_databases_list_json,
    control_api_host_db_columns_json, control_api_host_db_mongo_collections_json,
    control_api_host_db_mongo_databases_json, control_api_host_db_mongo_preview_json,
    control_api_host_db_query_json, control_api_host_db_redis_keys_json,
    control_api_host_db_relationships_json, control_api_host_db_rows_json,
    control_api_host_db_schemas_json, control_api_host_db_tables_json,
    control_api_host_db_v2_admin_create_database_json,
    control_api_host_db_v2_admin_create_table_json, control_api_host_db_v2_admin_create_user_json,
    control_api_host_db_v2_admin_delete_user_json, control_api_host_db_v2_capabilities_json,
    control_api_host_db_v2_grid_json, control_api_host_db_v2_migration_run_json,
    control_api_host_db_v2_migration_status_get_json,
    control_api_host_db_v2_migration_status_post_json, control_api_host_db_v2_object_tree_json,
    control_api_host_db_v2_row_mutate_json, control_api_host_db_v2_sql_job_cancel_json,
    control_api_host_db_v2_sql_job_get_json, control_api_host_db_v2_sql_job_start_json,
    control_api_host_service_install, control_api_host_service_remove,
    control_api_host_service_restart, control_api_host_service_runtime_get_json,
    control_api_host_service_runtime_put_json, control_api_kill_process_listener_json,
    control_api_login, control_api_logout, control_api_nginx_action_json,
    control_api_nginx_preflight_json, control_api_put_app_env, control_api_put_host_deploy_env,
    control_api_put_nginx_file_json, control_api_put_nginx_site, control_api_restart_process_json,
    control_api_session_active, control_api_stop_process_json, control_api_storage_bind_json,
    control_api_storage_bind_sources_json, control_api_storage_create_folder,
    control_api_storage_delete_file, control_api_storage_delete_folder,
    control_api_storage_download_file, control_api_storage_extract_json,
    control_api_storage_rename, control_api_storage_tree_json, control_api_storage_unbind_json,
    control_api_storage_upload_file, control_api_storage_usage_json,
    ensure_deploy_project_id_for_deploy, fetch_server_projects_overview,
    format_control_api_http_error, write_pirate_toml_deploy_project_id, ServerProjectRow,
    ServerProjectsOverview, StorageBindActive, StorageBindMountCandidate, StorageBindSourcesView,
    StorageEntryView, StorageListView, StorageUsageView,
};
pub use control_api_keychain::{
    control_api_keychain_delete, control_api_keychain_load, control_api_keychain_save,
    ControlApiKeychainCreds,
};
pub use db_tunnel::{
    db_local_forward_local_port, db_local_forward_start, db_local_forward_stop,
    db_tunnel_list_json, db_tunnel_ssh_start, db_tunnel_ssh_stop, db_tunnel_tcp_start,
    db_tunnel_tcp_stop,
};
pub use deploy::{
    analyze_network_access, check_project_uploaded, load_project_network_manifest,
    read_release_version_from_manifest, save_project_network_manifest,
    validate_network_access_remote, AnalyzeNetworkAccessOverrides, DeployOutcome,
    DeployProgressEvent, LoadProjectNetworkManifestView, NetworkAccessAnalysis,
    NetworkAccessRouteOverride, ProjectDeployCheck, RemoveProjectOutcome, RollbackOutcome,
    SaveProjectNetworkManifestInput,
};
pub use host_agent::{
    host_agent_health_json, host_agent_reboot_json, host_agent_status_json,
    host_agent_upload_server_stack,
};
pub use host_services_compat::{
    host_services_gap, summarize_host_services_for_manifest, HostServicesCompatSummary,
};
pub use local_dev_stack::{
    local_dev_status, start_local_dev_stack, stop_local_dev_stack, LocalDevLogLine, LocalDevStatus,
    LocalDevStream,
};
pub use manifest_fix::apply_manifest_fix;
pub use preflight::{run_projects_preflight, ProjectsPreflightReport};
pub use project_registry::{
    list_registered_projects, record_deploy_for_directory, register_project_from_directory,
    remove_registered_project, RegisteredProject,
};
pub use toolchain_probe::{probe_local_toolchain, ToolchainItem, ToolchainReport};

/// Best-effort: stop streaming chunks for an in-flight deploy upload (UI cancel).
pub fn deploy_upload_cancel() {
    deploy_client::set_artifact_upload_cancel(true);
}

/// Best-effort: stop streaming chunks for an in-flight server-stack upload (UI cancel).
pub fn server_stack_upload_cancel() {
    deploy_client::set_server_stack_upload_cancel(true);
}
pub use cmd_vars::{cmd_vars_map_from_json, project_cmd_placeholders};
pub use db_credentials::{db_credentials_forget, db_credentials_get_json, db_credentials_save};
pub use db_direct::{
    direct_close, direct_heartbeat, direct_list_databases, direct_list_schemas, direct_list_tables,
    direct_open, direct_password_set, direct_pg_stats_json, direct_pg_structure_json,
    direct_profile_delete, direct_profile_list_json, direct_profile_upsert, direct_query,
    direct_table_preview, direct_test, query_history_list_json, DirectOpenRequest,
    DirectPreviewRequest, DirectProfileUpsert, DirectQueryRequest, DirectStructureRequest,
    DirectTestRequest, DirectTestResponse,
};
pub use deploy_client::internet_proxy::ProxyTraceEntry;
pub use deploy_core::cmd_template::CmdPlaceholder;
pub use deploy_core::display_stream;
pub use display_ingest::{
    display_ingest_api_base, display_ingest_url, spawn_display_ingest_server,
};
pub use display_stream_prefs::{
    get_prefs as get_display_stream_prefs, set_prefs as set_display_stream_prefs,
};
pub use host_stats::{fetch_host_stats_detail_json, fetch_host_stats_json};
pub use internet_proxy::{
    apply_default_rules_preset, apply_default_rules_preset_to_disk, default_listen_addr,
    internet_proxy_logs, internet_proxy_logs_clear, internet_proxy_start, internet_proxy_status,
    internet_proxy_stop, load_board_rules_form, load_default_rules_bundles_form,
    load_settings_json, save_board_rules_form, save_default_rules_bundles_form, save_settings_json,
    BoardRulesForm, DefaultRulesBundlesForm, InternetProxyStatus,
};
pub use monitoring::{monitoring_api_base, monitoring_set_economy_mode, spawn_monitoring_server};
pub use paas::{
    run_apply_gen, run_init_project, run_pipeline, run_project_build, run_project_test,
    run_scan_project, run_test_local, PipelineOutcome,
};
pub use server_stack::{
    fetch_server_stack_info_json, run_server_stack_update_with_progress, ServerStackOutcome,
};
pub use ssl_ops::{ssl_check_and_renew_json, ssl_create_json, ssl_status_json, ssl_update_json};
pub use stack_tun_remote::{
    normalize_stack_tun_base, stack_tun_authorize_peer_json, stack_tun_get_config_json,
    stack_tun_get_routes_json, stack_tun_health, stack_tun_identity_public_key_json,
    stack_tun_list_peers_json, stack_tun_put_config_json, stack_tun_put_routes_json,
    stack_tun_reload_peers, stack_tun_request_bus_invoke_json, stack_tun_requests_json,
    stack_tun_stats_json,
};
pub use perf::desktop_perf_snapshot_json;
pub use status::{app_status, AppStatus};
