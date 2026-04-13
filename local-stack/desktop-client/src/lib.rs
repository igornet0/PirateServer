//! Pirate Client desktop library (Tauri backend helpers).

mod desktop_store;
mod display_stream_prefs;
pub mod internet_proxy;
pub mod display_ingest;
pub mod bookmarks;
pub mod connection;
pub mod deploy;
pub mod server_stack;
pub mod host_stats;
pub mod hosts;
pub mod monitoring;
pub mod status;

pub use bookmarks::{
    load_bookmarks, remove_bookmark, set_bookmark_label, upsert_bookmark, ServerBookmark,
};
pub use connection::{
    clear_endpoint, connect_from_bundle, load_endpoint, load_project_id,
    load_control_api_base, parse_grpc_endpoint_from_bundle, save_endpoint, set_active_project,
    set_control_api_base, verify_grpc_endpoint, GrpcConnectResult,
};
pub use deploy::{DeployOutcome, RollbackOutcome};
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
