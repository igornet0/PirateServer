//! Pirate Client — Tauri shell (embedded WebView, no loopback HTTP server).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
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
fn activate_server_bookmark(url: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::connection::activate_bookmark_url(&url)
}

fn main() {
    let _guard = init_tracing();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_status,
            parse_grpc_bundle,
            connect_grpc_bundle,
            get_saved_grpc_endpoint,
            clear_grpc_connection,
            test_grpc_endpoint,
            refresh_grpc_status,
            get_active_project,
            set_active_project,
            pick_deploy_directory,
            deploy_from_directory,
            rollback_deploy,
            list_server_bookmarks,
            delete_server_bookmark,
            activate_server_bookmark,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
