//! Pirate Client — Tauri shell (embedded WebView, no loopback HTTP server).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::Duration;
use tauri::Emitter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(&log_dir, "pirate-client.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(true),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .init();

    tracing::info!(log_dir = %log_dir.display(), "logging to file and stdout");
    guard
}

#[tauri::command]
fn get_status() -> pirate_desktop::AppStatus {
    pirate_desktop::app_status()
}

#[tauri::command]
fn parse_grpc_bundle(bundle: String) -> Result<String, String> {
    pirate_desktop::parse_grpc_endpoint_from_bundle(&bundle)
}

#[tauri::command]
fn connect_grpc_bundle(bundle: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::connect_from_bundle(&bundle)
}

#[tauri::command]
fn get_saved_grpc_endpoint() -> Option<String> {
    pirate_desktop::load_endpoint()
}

#[tauri::command]
fn clear_grpc_connection() -> Result<(), String> {
    pirate_desktop::clear_endpoint()
}

#[tauri::command]
fn test_grpc_endpoint(endpoint: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::verify_grpc_endpoint(&endpoint)
}

#[tauri::command]
fn refresh_grpc_status() -> Result<pirate_desktop::GrpcConnectResult, String> {
    let ep =
        pirate_desktop::load_endpoint().ok_or_else(|| "no saved connection".to_string())?;
    pirate_desktop::verify_grpc_endpoint(&ep)
}

#[tauri::command]
fn get_control_api_base() -> Option<String> {
    pirate_desktop::load_control_api_base()
}

#[tauri::command]
fn set_control_api_base(url: String) -> Result<(), String> {
    pirate_desktop::set_control_api_base(&url)
}

#[tauri::command]
fn fetch_remote_host_stats() -> Result<String, String> {
    pirate_desktop::fetch_host_stats_json()
}

#[tauri::command]
fn fetch_remote_host_stats_detail(
    kind: i32,
    top: u32,
    q: String,
    limit: u32,
) -> Result<String, String> {
    pirate_desktop::fetch_host_stats_detail_json(kind, top, q, limit)
}

/// `GET {base}/api/v1/host-stats/series` for `net_rx` and `net_tx` (control-api; requires
/// `CONTROL_API_HOST_STATS_SERIES=1` on the server). Same base URL as gRPC endpoint (HTTPS).
#[tauri::command]
async fn fetch_remote_host_stats_series(base_url: String, range: String) -> Result<String, String> {
    fn norm_range(s: &str) -> &'static str {
        let r = s.trim().to_lowercase().replace(' ', "");
        match r.as_str() {
            "15m" | "15min" => "15m",
            "1h" | "60m" | "60min" => "1h",
            "24h" | "24hr" | "1d" | "1440m" => "24h",
            "7d" | "1w" | "week" | "168h" | "168hr" => "7d",
            _ => "1h",
        }
    }

    let base = base_url.trim().trim_end_matches('/');
    let enc = norm_range(&range);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;

    let rx_url = format!("{base}/api/v1/host-stats/series?metric=net_rx&range={enc}");
    let tx_url = format!("{base}/api/v1/host-stats/series?metric=net_tx&range={enc}");

    let (rx_res, tx_res) = tokio::join!(client.get(&rx_url).send(), client.get(&tx_url).send(),);

    let rx_res = rx_res.map_err(|e| e.to_string())?;
    let tx_res = tx_res.map_err(|e| e.to_string())?;

    let rx_status = rx_res.status();
    let tx_status = tx_res.status();
    let rx_body = rx_res.text().await.map_err(|e| e.to_string())?;
    let tx_body = tx_res.text().await.map_err(|e| e.to_string())?;

    if !rx_status.is_success() {
        return Err(format!(
            "net_rx HTTP {}: {}",
            rx_status,
            rx_body.chars().take(200).collect::<String>()
        ));
    }
    if !tx_status.is_success() {
        return Err(format!(
            "net_tx HTTP {}: {}",
            tx_status,
            tx_body.chars().take(200).collect::<String>()
        ));
    }

    let net_rx: serde_json::Value = serde_json::from_str(&rx_body).map_err(|e| e.to_string())?;
    let net_tx: serde_json::Value = serde_json::from_str(&tx_body).map_err(|e| e.to_string())?;
    let out = serde_json::json!({ "net_rx": net_rx, "net_tx": net_tx });
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_active_project() -> String {
    pirate_desktop::load_project_id()
}

#[tauri::command]
fn set_active_project(project_id: String) -> Result<(), String> {
    pirate_desktop::set_active_project(project_id)
}

#[tauri::command]
fn pick_deploy_directory() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn deploy_from_directory(
    directory: String,
    version: String,
    chunk_size: Option<u32>,
) -> Result<pirate_desktop::DeployOutcome, String> {
    let chunk = chunk_size.unwrap_or(64 * 1024) as usize;
    pirate_desktop::deploy::run_deploy(
        std::path::PathBuf::from(directory),
        version,
        chunk,
    )
}

#[tauri::command]
fn rollback_deploy(version: String) -> Result<pirate_desktop::RollbackOutcome, String> {
    pirate_desktop::deploy::run_rollback(version)
}

#[tauri::command]
fn list_server_bookmarks() -> Vec<pirate_desktop::ServerBookmark> {
    pirate_desktop::load_bookmarks()
}

#[tauri::command]
fn delete_server_bookmark(id: String) -> Result<(), String> {
    pirate_desktop::remove_bookmark(&id)
}

#[tauri::command]
fn add_server_bookmark(url: String) -> Result<pirate_desktop::ServerBookmark, String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("URL is empty".into());
    }
    let id = pirate_desktop::upsert_bookmark(u, u)?;
    pirate_desktop::load_bookmarks()
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| "bookmark not found after insert".to_string())
}

#[tauri::command]
fn activate_server_bookmark(url: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::connection::activate_bookmark_url(&url)
}

#[tauri::command]
fn rename_server_bookmark(id: String, label: String) -> Result<(), String> {
    pirate_desktop::set_bookmark_label(&id, label)
}

#[tauri::command]
fn monitoring_api_base() -> Option<String> {
    pirate_desktop::monitoring_api_base()
}

#[tauri::command]
fn monitoring_set_economy(enabled: bool) -> bool {
    pirate_desktop::monitoring_set_economy_mode(enabled)
}

#[tauri::command]
fn start_display_ingest(token: Option<String>) -> Result<u16, String> {
    pirate_desktop::spawn_display_ingest_server(token).map_err(|e| e.to_string())
}

#[tauri::command]
fn display_ingest_base() -> Option<String> {
    pirate_desktop::display_ingest_api_base()
}

/// `data:application/json;base64,...` for consumer role (paste / share with producer host).
#[tauri::command]
fn display_ingest_export_consumer_config(token: Option<String>) -> Result<String, String> {
    let base = pirate_desktop::display_ingest_api_base()
        .ok_or_else(|| "start display ingest first".to_string())?;
    let url = format!("{}/ingest", base.trim_end_matches('/'));
    let mut cfg = deploy_core::display_stream::DisplayStreamConfig::example_consumer(&url);
    if let Some(t) = token.filter(|s| !s.trim().is_empty()) {
        cfg.token = t;
    }
    cfg.to_data_url().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_display_stream_prefs() -> [bool; 2] {
    let (a, b) = pirate_desktop::get_display_stream_prefs();
    [a, b]
}

#[tauri::command]
fn set_display_stream_prefs(allow_receive: bool, allow_send: bool) -> Result<(), String> {
    pirate_desktop::set_display_stream_prefs(allow_receive, allow_send)
}

#[tauri::command]
fn internet_proxy_start(listen: Option<String>) -> Result<(), String> {
    pirate_desktop::internet_proxy_start(listen)
}

#[tauri::command]
fn internet_proxy_stop() -> Result<(), String> {
    pirate_desktop::internet_proxy_stop()
}

#[tauri::command]
fn internet_proxy_status() -> pirate_desktop::InternetProxyStatus {
    pirate_desktop::internet_proxy_status()
}

#[tauri::command]
fn internet_proxy_logs() -> Vec<pirate_desktop::ProxyTraceEntry> {
    pirate_desktop::internet_proxy_logs()
}

#[tauri::command]
fn internet_proxy_logs_clear() {
    pirate_desktop::internet_proxy_logs_clear();
}

#[tauri::command]
fn load_client_settings_json() -> Result<String, String> {
    pirate_desktop::load_settings_json()
}

#[tauri::command]
fn save_client_settings_json(text: String) -> Result<(), String> {
    pirate_desktop::save_settings_json(&text)
}

#[tauri::command]
fn apply_default_rules_preset_cmd(preset: String) -> Result<(), String> {
    pirate_desktop::apply_default_rules_preset_to_disk(&preset)
}

#[tauri::command]
fn load_default_rules_bundles_form() -> Result<pirate_desktop::DefaultRulesBundlesForm, String> {
    pirate_desktop::load_default_rules_bundles_form()
}

#[tauri::command]
fn save_default_rules_bundles_form(
    form: pirate_desktop::DefaultRulesBundlesForm,
) -> Result<(), String> {
    pirate_desktop::save_default_rules_bundles_form(form)
}

#[tauri::command]
fn load_board_rules_form() -> Result<pirate_desktop::BoardRulesForm, String> {
    pirate_desktop::load_board_rules_form()
}

#[tauri::command]
fn save_board_rules_form(form: pirate_desktop::BoardRulesForm) -> Result<(), String> {
    pirate_desktop::save_board_rules_form(form)
}

#[tauri::command]
fn pick_server_stack_tar_gz() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .add_filter("Tarball", &["tar.gz", "tgz"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn fetch_server_stack_info_cmd() -> Result<String, String> {
    pirate_desktop::fetch_server_stack_info_json()
}

#[tauri::command]
async fn apply_server_stack_update(
    app: tauri::AppHandle,
    path: String,
    version: String,
    chunk_size: Option<u32>,
) -> Result<pirate_desktop::ServerStackOutcome, String> {
    let chunk = chunk_size.unwrap_or(64 * 1024) as usize;
    let path = PathBuf::from(path);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        pirate_desktop::run_server_stack_update_with_progress(path, version, chunk, move |sent, total| {
            let _ = app.emit(
                "server_stack_upload_progress",
                serde_json::json!({ "sent": sent, "total": total }),
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    let _guard = init_tracing();
    if let Err(e) = pirate_desktop::spawn_monitoring_server() {
        tracing::warn!(%e, "monitoring HTTP server not started");
    } else {
        tracing::info!(
            base = ?pirate_desktop::monitoring_api_base(),
            "monitoring API"
        );
    }
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_status,
            parse_grpc_bundle,
            connect_grpc_bundle,
            get_saved_grpc_endpoint,
            clear_grpc_connection,
            test_grpc_endpoint,
            refresh_grpc_status,
            get_control_api_base,
            set_control_api_base,
            get_active_project,
            set_active_project,
            pick_deploy_directory,
            deploy_from_directory,
            rollback_deploy,
            list_server_bookmarks,
            delete_server_bookmark,
            add_server_bookmark,
            activate_server_bookmark,
            rename_server_bookmark,
            monitoring_api_base,
            monitoring_set_economy,
            start_display_ingest,
            display_ingest_base,
            display_ingest_export_consumer_config,
            get_display_stream_prefs,
            set_display_stream_prefs,
            internet_proxy_start,
            internet_proxy_stop,
            internet_proxy_status,
            internet_proxy_logs,
            internet_proxy_logs_clear,
            load_client_settings_json,
            save_client_settings_json,
            apply_default_rules_preset_cmd,
            load_default_rules_bundles_form,
            save_default_rules_bundles_form,
            load_board_rules_form,
            save_board_rules_form,
            fetch_remote_host_stats,
            fetch_remote_host_stats_detail,
            fetch_remote_host_stats_series,
            pick_server_stack_tar_gz,
            fetch_server_stack_info_cmd,
            apply_server_stack_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
