//! Pirate Client desktop library (Tauri backend helpers).

mod desktop_store;
mod display_stream_prefs;
mod env_paths;
mod host_services_compat;
mod preflight;
pub mod internet_proxy;
pub mod display_ingest;
pub mod bookmarks;
pub mod connection;
pub mod control_api;
pub mod host_agent;
pub mod deploy;
pub mod paas;
pub mod server_stack;
pub mod host_stats;
pub mod hosts;
pub mod monitoring;
pub mod project_registry;
pub mod local_dev_stack;
pub mod toolchain_probe;
pub mod status;

pub use bookmarks::{
    bookmark_by_id, load_bookmarks, remove_bookmark, set_bookmark_host_agent, set_bookmark_label,
    upsert_bookmark, ServerBookmark,
};
pub use connection::{
    add_bookmark_from_input, clear_endpoint, connect_from_bundle, load_endpoint, load_project_id,
    control_api_recent_restart_hint,
    load_control_api_base, parse_grpc_endpoint_from_bundle, save_endpoint, set_active_project,
    mark_control_api_recent_restart, set_control_api_base,     verify_grpc_endpoint,
    verify_grpc_status_for_project, verify_grpc_status_for_project_async, GrpcConnectResult,
};
pub use host_agent::{
    host_agent_health_json, host_agent_reboot_json, host_agent_status_json,
    host_agent_upload_server_stack,
};
pub use control_api::{
    allocate_and_apply_remote_project_id, allocate_remote_project_id, control_api_ensure_nginx,
    control_api_antiddos_apply, control_api_antiddos_disable, control_api_antiddos_enable,
    control_api_antiddos_get_json, control_api_antiddos_project_delete,
    control_api_antiddos_project_put_json, control_api_antiddos_put_json,
    control_api_antiddos_stats_json,
    control_api_fetch_app_env_json, control_api_fetch_host_deploy_env_json,
    control_api_fetch_host_deploy_env_template_json, control_api_fetch_host_services_json,
    control_api_fetch_nginx_site_json, control_api_fetch_nginx_status_json,
    control_api_clear_project_runtime_log, control_api_fetch_project_telemetry_json,
    control_api_health_probe,
    control_api_fetch_status_json,
    control_api_host_service_install, control_api_host_service_remove, control_api_bearer_token,
    control_api_login,
    control_api_logout, control_api_put_app_env, control_api_put_host_deploy_env,
    control_api_put_nginx_site, control_api_restart_process_json, control_api_session_active,
    control_api_stop_process_json, ensure_deploy_project_id_for_deploy,
    fetch_server_projects_overview, write_pirate_toml_deploy_project_id, ServerProjectRow,
    ServerProjectsOverview,
};
pub use deploy::{
    analyze_network_access, check_project_uploaded, read_release_version_from_manifest,
    validate_network_access_remote, AnalyzeNetworkAccessOverrides, DeployOutcome, DeployProgressEvent,
    NetworkAccessAnalysis, NetworkAccessRouteOverride, ProjectDeployCheck, RemoveProjectOutcome,
    RollbackOutcome,
};
pub use host_services_compat::{host_services_gap, summarize_host_services_for_manifest, HostServicesCompatSummary};
pub use preflight::{run_projects_preflight, ProjectsPreflightReport};
pub use project_registry::{
    list_registered_projects, register_project_from_directory, remove_registered_project,
    RegisteredProject,
};
pub use local_dev_stack::{
    local_dev_status, start_local_dev_stack, stop_local_dev_stack, LocalDevLogLine, LocalDevStatus,
    LocalDevStream,
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
pub use paas::{
    run_apply_gen, run_init_project, run_pipeline, run_project_build, run_project_test,
    run_scan_project, run_test_local, PipelineOutcome,
};
pub use server_stack::{fetch_server_stack_info_json, run_server_stack_update_with_progress, ServerStackOutcome};
pub use status::{app_status, AppStatus};
pub use host_stats::{fetch_host_stats_detail_json, fetch_host_stats_json};
pub use monitoring::{monitoring_api_base, monitoring_set_economy_mode, spawn_monitoring_server};
pub use deploy_core::display_stream;
pub use display_ingest::{display_ingest_api_base, display_ingest_url, spawn_display_ingest_server};
pub use display_stream_prefs::{get_prefs as get_display_stream_prefs, set_prefs as set_display_stream_prefs};
pub use internet_proxy::{
    apply_default_rules_preset, apply_default_rules_preset_to_disk, default_listen_addr,
    internet_proxy_logs, internet_proxy_logs_clear, internet_proxy_start, internet_proxy_status,
    internet_proxy_stop, load_board_rules_form, load_default_rules_bundles_form, load_settings_json,
    save_board_rules_form, save_default_rules_bundles_form, save_settings_json, BoardRulesForm,
    DefaultRulesBundlesForm, InternetProxyStatus,
};
pub use deploy_client::internet_proxy::ProxyTraceEntry;
