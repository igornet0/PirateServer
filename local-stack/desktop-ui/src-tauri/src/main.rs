//! Pirate Client — Tauri shell (embedded WebView, no loopback HTTP server).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager};
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

/// True if `pirate` (CLI from `deploy-client`) resolves in PATH (terminal).
#[tauri::command]
fn is_pirate_cli_available() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "where", "pirate"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v pirate >/dev/null 2>&1")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn verify_cli_blob(p: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    let len = meta.len();
    if len < 4096 {
        return Err(format!(
            "Встроенный CLI не собран ({} байт, путь: {}). Сначала выполните: cargo build -p deploy-client --bin pirate, затем пересоберите клиент (tauri build).",
            len,
            p.display()
        ));
    }
    Ok(())
}

fn resolve_pirate_cli_source(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = app.path().resolve("bundled/cli/pirate", BaseDirectory::Resource) {
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(rd) = app.path().resource_dir() {
        let p = rd.join("bundled/cli/pirate");
        if p.is_file() {
            return Ok(p);
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "нет каталога исполняемого файла".to_string())?;
    #[cfg(windows)]
    let side = dir.join("pirate.exe");
    #[cfg(not(windows))]
    let side = dir.join("pirate");
    if side.is_file() {
        return Ok(side);
    }
    Err("Не найден встроенный бинарь pirate (bundled/cli/pirate).".into())
}

#[cfg(target_os = "linux")]
fn install_pirate_to_path_linux(src: &Path) -> Result<String, String> {
    let src_s = src.to_str().ok_or_else(|| "некорректный путь".to_string())?;
    let st = std::process::Command::new("pkexec")
        .args(["install", "-m", "0755", src_s, "/usr/local/bin/pirate"])
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        return Ok(
            "pirate установлен в /usr/local/bin/pirate. Откройте новый терминал и проверьте: pirate --help"
                .into(),
        );
    }
    Err(
        "Не удалось установить через pkexec. Запустите вручную: sudo install -m 0755 <путь> /usr/local/bin/pirate"
            .into(),
    )
}

#[cfg(target_os = "macos")]
fn install_pirate_to_path_macos(src: &Path) -> Result<String, String> {
    let cmd = format!(
        "/bin/mkdir -p /usr/local/bin && /usr/bin/install -m 0755 {} /usr/local/bin/pirate",
        sh_single_quote(&src.to_string_lossy())
    );
    let script = format!(
        "do shell script {} with administrator privileges",
        serde_json::to_string(&cmd).map_err(|e| e.to_string())?
    );
    let out = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(
            "pirate установлен в /usr/local/bin/pirate. Откройте новый терминал.".into(),
        );
    }
    Err(format!(
        "Ошибка: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

#[cfg(target_os = "windows")]
fn install_pirate_to_path_windows(src: &Path) -> Result<String, String> {
    let src_s = src.to_string_lossy().to_string();
    let ps = format!(
        r#"$ErrorActionPreference = 'Stop'
$src = {src_json}
$destDir = Join-Path $env:LOCALAPPDATA 'PirateClient\bin'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
Copy-Item -LiteralPath $src -Destination (Join-Path $destDir 'pirate.exe') -Force
$u = [Environment]::GetEnvironmentVariable('Path','User')
if ($null -eq $u) {{ $u = '' }}
if ($u -notlike "*$destDir*") {{
  [Environment]::SetEnvironmentVariable('Path', ($u.TrimEnd(';') + ';' + $destDir), 'User')
}}
Write-Output 'OK'
"#,
        src_json = serde_json::to_string(&src_s).map_err(|e| e.to_string())?,
    );
    let tmp = std::env::temp_dir().join(format!("pirate-install-{}.ps1", std::process::id()));
    std::fs::write(&tmp, ps.as_bytes()).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            tmp.to_str().ok_or_else(|| "temp path".to_string())?,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        let err = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
        return Err(if err.is_empty() {
            "PowerShell завершился с ошибкой.".into()
        } else {
            err
        });
    }
    Ok(
        "pirate.exe установлен в %LOCALAPPDATA%\\PirateClient\\bin и добавлен в PATH пользователя. Закройте и снова откройте терминал, затем: pirate --help"
            .into(),
    )
}

fn install_pirate_cli_sync(app: tauri::AppHandle) -> Result<String, String> {
    let src = resolve_pirate_cli_source(&app)?;
    verify_cli_blob(&src)?;

    #[cfg(target_os = "linux")]
    return install_pirate_to_path_linux(&src);

    #[cfg(target_os = "macos")]
    return install_pirate_to_path_macos(&src);

    #[cfg(target_os = "windows")]
    return install_pirate_to_path_windows(&src);

    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
    {
        Err("Установка CLI на этой ОС не поддерживается.".into())
    }
}

/// Копирует встроенный `pirate` в каталог из PATH (ОС: macOS → /usr/local/bin, Linux → pkexec, Windows → %LOCALAPPDATA%\\PirateClient\\bin + user PATH).
#[tauri::command]
async fn install_pirate_cli(app: tauri::AppHandle) -> Result<String, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || install_pirate_cli_sync(app))
        .await
        .map_err(|e| e.to_string())?
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
fn read_release_version_from_manifest(directory: String) -> Result<String, String> {
    pirate_desktop::read_release_version_from_manifest(PathBuf::from(directory).as_path())
}

#[tauri::command]
fn check_project_uploaded(directory: String) -> Result<pirate_desktop::ProjectDeployCheck, String> {
    pirate_desktop::check_project_uploaded(PathBuf::from(directory))
}

#[tauri::command]
fn remove_server_project(project_id: String) -> Result<pirate_desktop::RemoveProjectOutcome, String> {
    pirate_desktop::deploy::run_remove_project(project_id)
}

#[tauri::command]
fn analyze_network_access(directory: String) -> Result<pirate_desktop::NetworkAccessAnalysis, String> {
    pirate_desktop::analyze_network_access(PathBuf::from(directory))
}

#[tauri::command]
fn validate_network_access(directory: String) -> Result<String, String> {
    let r = pirate_desktop::validate_network_access_remote(PathBuf::from(directory))?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn projects_preflight(directory: String, version: String) -> Result<String, String> {
    let report = pirate_desktop::run_projects_preflight(PathBuf::from(directory), &version);
    serde_json::to_string(&report).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_registered_projects() -> Result<Vec<pirate_desktop::RegisteredProject>, String> {
    pirate_desktop::list_registered_projects()
}

#[tauri::command]
fn register_project_from_directory(path: String) -> Result<String, String> {
    pirate_desktop::register_project_from_directory(path)
}

#[tauri::command]
fn remove_registered_project(name: String) -> Result<bool, String> {
    pirate_desktop::remove_registered_project(name)
}

#[tauri::command]
fn local_dev_start(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let app = app.clone();
    let emit: std::sync::Arc<dyn Fn(pirate_desktop::LocalDevLogLine) + Send + Sync> =
        std::sync::Arc::new(move |line: pirate_desktop::LocalDevLogLine| {
            let _ = app.emit("local-dev-log", &line);
        });
    pirate_desktop::start_local_dev_stack(PathBuf::from(path), Some(emit))
}

#[tauri::command]
fn local_dev_stop() -> Result<(), String> {
    pirate_desktop::stop_local_dev_stack()
}

#[tauri::command]
fn local_dev_status() -> pirate_desktop::LocalDevStatus {
    pirate_desktop::local_dev_status()
}

#[tauri::command]
fn probe_local_toolchain() -> pirate_desktop::ToolchainReport {
    pirate_desktop::probe_local_toolchain()
}

#[tauri::command]
fn control_api_login(base_url: String, username: String, password: String) -> Result<(), String> {
    pirate_desktop::control_api_login(&base_url, &username, &password)
}

#[tauri::command]
fn control_api_logout() -> Result<(), String> {
    pirate_desktop::control_api_logout()
}

#[tauri::command]
fn control_api_session_active() -> bool {
    pirate_desktop::control_api_session_active()
}

#[tauri::command]
fn control_api_fetch_status_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_fetch_status_json(&project_id)
}

#[tauri::command]
fn control_api_fetch_project_telemetry_json(
    project_id: String,
    logs_limit: Option<usize>,
) -> Result<String, String> {
    pirate_desktop::control_api_fetch_project_telemetry_json(&project_id, logs_limit.unwrap_or(120))
}

#[tauri::command]
fn control_api_clear_project_runtime_log(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_clear_project_runtime_log(&project_id)
}

#[tauri::command]
fn control_api_fetch_app_env_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_fetch_app_env_json(&project_id)
}

#[tauri::command]
fn control_api_put_app_env(project_id: String, content: String) -> Result<(), String> {
    pirate_desktop::control_api_put_app_env(&project_id, &content)
}

#[tauri::command]
fn control_api_fetch_host_deploy_env_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_host_deploy_env_json()
}

#[tauri::command]
fn control_api_put_host_deploy_env(content: String) -> Result<String, String> {
    pirate_desktop::control_api_put_host_deploy_env(&content)
}

#[tauri::command]
fn control_api_fetch_host_deploy_env_template_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_host_deploy_env_template_json()
}

#[tauri::command]
fn control_api_fetch_nginx_status_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_nginx_status_json()
}

#[tauri::command]
fn control_api_fetch_host_services_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_host_services_json()
}

#[tauri::command]
fn control_api_host_service_install(id: String) -> Result<String, String> {
    pirate_desktop::control_api_host_service_install(&id)
}

#[tauri::command]
fn control_api_host_service_remove(id: String) -> Result<String, String> {
    pirate_desktop::control_api_host_service_remove(&id)
}

#[tauri::command]
fn control_api_fetch_nginx_site_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_nginx_site_json()
}

#[tauri::command]
fn control_api_put_nginx_site(content: String) -> Result<String, String> {
    pirate_desktop::control_api_put_nginx_site(&content)
}

#[tauri::command]
fn control_api_ensure_nginx(mode: String) -> Result<String, String> {
    pirate_desktop::control_api_ensure_nginx(&mode)
}

#[tauri::command]
fn control_api_restart_process_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_restart_process_json(&project_id)
}

#[tauri::command]
fn control_api_stop_process_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_stop_process_json(&project_id)
}

#[tauri::command]
fn control_api_antiddos_get_json() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_get_json()
}

#[tauri::command]
fn control_api_antiddos_put_json(content: String) -> Result<String, String> {
    pirate_desktop::control_api_antiddos_put_json(&content)
}

#[tauri::command]
fn control_api_antiddos_enable() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_enable()
}

#[tauri::command]
fn control_api_antiddos_disable() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_disable()
}

#[tauri::command]
fn control_api_antiddos_apply() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_apply()
}

#[tauri::command]
fn control_api_antiddos_stats_json() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_stats_json()
}

#[tauri::command]
fn control_api_antiddos_project_put_json(project_id: String, content: String) -> Result<String, String> {
    pirate_desktop::control_api_antiddos_project_put_json(&project_id, &content)
}

#[tauri::command]
fn control_api_antiddos_project_delete(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_antiddos_project_delete(&project_id)
}

#[tauri::command]
fn fetch_server_projects_overview() -> Result<pirate_desktop::ServerProjectsOverview, String> {
    pirate_desktop::fetch_server_projects_overview()
}

#[tauri::command]
fn ensure_deploy_project_id_for_deploy(path: String) -> Result<String, String> {
    pirate_desktop::ensure_deploy_project_id_for_deploy(PathBuf::from(path))
}

/// Open a project directory in the system file manager (Finder, Explorer, …).
#[tauri::command]
fn open_project_folder(path: String) -> Result<(), String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("path is empty".into());
    }
    let pb = PathBuf::from(p);
    if !pb.is_dir() {
        return Err(format!("not a directory: {}", pb.display()));
    }
    open::that(&pb).map_err(|e| e.to_string())
}

#[tauri::command]
fn deploy_upload_cancel() {
    pirate_desktop::deploy_upload_cancel();
}

#[tauri::command]
fn server_stack_upload_cancel() {
    pirate_desktop::server_stack_upload_cancel();
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
    let id = pirate_desktop::add_bookmark_from_input(&url)?;
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
        // Native dialogs usually filter by final extension, so include "gz" too.
        .add_filter("Tarball", &["tar", "tgz", "gz"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn fetch_server_stack_info_cmd() -> Result<String, String> {
    pirate_desktop::fetch_server_stack_info_json()
}

#[tauri::command]
fn paas_init_project(path: String, name: Option<String>) -> Result<String, String> {
    pirate_desktop::run_init_project(PathBuf::from(path), name)
}

#[tauri::command]
fn paas_scan_project(path: String, dry_run: bool) -> Result<String, String> {
    let r = pirate_desktop::run_scan_project(PathBuf::from(path), dry_run)?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_project_build(path: String) -> Result<String, String> {
    let r = pirate_desktop::run_project_build(PathBuf::from(path))?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_project_test(path: String) -> Result<String, String> {
    let r = pirate_desktop::run_project_test(PathBuf::from(path))?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_test_local(path: String, image: Option<String>) -> Result<String, String> {
    let r = pirate_desktop::run_test_local(
        PathBuf::from(path),
        image.unwrap_or_else(|| "pirate-local-test".to_string()),
    )?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_apply_gen(path: String) -> Result<(), String> {
    pirate_desktop::run_apply_gen(PathBuf::from(path))
}

#[tauri::command]
fn paas_pipeline(
    path: String,
    do_init: bool,
    name: Option<String>,
    skip_test_local: bool,
    version: Option<String>,
    chunk_size: Option<u32>,
) -> Result<String, String> {
    let r = pirate_desktop::run_pipeline(
        PathBuf::from(path),
        do_init,
        name,
        skip_test_local,
        version,
        chunk_size.unwrap_or(64 * 1024) as usize,
    )?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
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
            is_pirate_cli_available,
            install_pirate_cli,
            get_status, // app status
            parse_grpc_bundle, // parse install JSON from bundle
            connect_grpc_bundle, // connect from bundle
            get_saved_grpc_endpoint, // saved gRPC endpoint
            clear_grpc_connection, // clear saved gRPC endpoint
            test_grpc_endpoint, // test gRPC endpoint
            refresh_grpc_status, // refresh gRPC endpoint status
            get_control_api_base, // control-api base URL
            set_control_api_base, // set control-api base URL
            get_active_project, // active project ID
            set_active_project, // set active project ID
            pick_deploy_directory, // pick deploy directory
            deploy_from_directory, // deploy from directory
            rollback_deploy, // rollback deploy
            read_release_version_from_manifest, // read [project].version from pirate.toml
            check_project_uploaded, // project deployed status for selected path
            remove_server_project, // remove project on server (stop process + delete files + db rows)
            analyze_network_access, // local detect services + nginx preview from manifest
            validate_network_access, // server-side deploy validation blockers/warnings
            projects_preflight, // projects preflight
            list_registered_projects, // list registered projects
            register_project_from_directory, // register project from directory
            remove_registered_project, // remove registered project
            local_dev_start, // local stack: compose + start.cmd
            local_dev_stop, // local stack: compose + stop.cmd
            local_dev_status, // local stack: status
            probe_local_toolchain, // local CLI toolchain probe
            control_api_login, // control-api JWT login
            control_api_logout, // clear control-api JWT
            control_api_session_active, // JWT present and not expired (for UI)
            control_api_fetch_status_json, // GET /api/v1/status (JWT)
            control_api_fetch_project_telemetry_json, // GET /api/v1/projects/telemetry (JWT)
            control_api_clear_project_runtime_log, // POST /api/v1/projects/telemetry/clear (JWT)
            control_api_fetch_app_env_json, // GET /api/v1/app-env (JWT)
            control_api_put_app_env, // PUT /api/v1/app-env (JWT)
            control_api_fetch_host_deploy_env_json, // GET /api/v1/host-deploy-env (JWT)
            control_api_put_host_deploy_env, // PUT /api/v1/host-deploy-env (JWT)
            control_api_fetch_host_deploy_env_template_json, // GET reference env.example (JWT)
            control_api_fetch_nginx_status_json, // GET /api/v1/nginx/status (JWT)
            control_api_fetch_host_services_json, // GET /api/v1/host-services (JWT)
            control_api_host_service_install, // POST /api/v1/host-services/:id/install (JWT)
            control_api_host_service_remove, // POST /api/v1/host-services/:id/remove (JWT)
            control_api_fetch_nginx_site_json, // GET /api/v1/nginx/site (JWT)
            control_api_put_nginx_site, // PUT /api/v1/nginx/site (JWT)
            control_api_ensure_nginx, // POST /api/v1/nginx/ensure (JWT)
            control_api_restart_process_json, // POST /api/v1/process/restart (JWT)
            control_api_stop_process_json, // POST /api/v1/process/stop (JWT)
            control_api_antiddos_get_json,
            control_api_antiddos_put_json,
            control_api_antiddos_enable,
            control_api_antiddos_disable,
            control_api_antiddos_apply,
            control_api_antiddos_stats_json,
            control_api_antiddos_project_put_json,
            control_api_antiddos_project_delete,
            fetch_server_projects_overview, // projects list + per-project status
            ensure_deploy_project_id_for_deploy, // resolve deploy slot (default vs allocate) before deploy
            open_project_folder, // reveal project folder in file manager
            deploy_upload_cancel, // cancel deploy upload
            server_stack_upload_cancel, // cancel server stack upload
            list_server_bookmarks, // list server bookmarks
            delete_server_bookmark, // delete server bookmark
            add_server_bookmark, // add server bookmark
            activate_server_bookmark, // activate server bookmark
            rename_server_bookmark, // rename server bookmark
            monitoring_api_base, // monitoring API base URL
            monitoring_set_economy, // set monitoring economy mode
            start_display_ingest, // start display ingest
            display_ingest_base, // display ingest base URL
            display_ingest_export_consumer_config, // display ingest export consumer config
            get_display_stream_prefs, // get display stream prefs
            set_display_stream_prefs, // set display stream prefs
            internet_proxy_start, // start internet proxy
            internet_proxy_stop, // stop internet proxy
            internet_proxy_status, // internet proxy status
            internet_proxy_logs, // internet proxy logs
            internet_proxy_logs_clear, // internet proxy logs clear
            load_client_settings_json, // load client settings
            save_client_settings_json, // save client settings
            apply_default_rules_preset_cmd, // apply default rules preset
            load_default_rules_bundles_form, // load default rules bundles form
            save_default_rules_bundles_form, // save default rules bundles form
            load_board_rules_form, // load board rules form
            save_board_rules_form, // save board rules form
            fetch_remote_host_stats, // fetch remote host stats
            fetch_remote_host_stats_detail, // fetch remote host stats detail
            fetch_remote_host_stats_series, // fetch remote host stats series
            pick_server_stack_tar_gz, // pick server stack tar.gz
            fetch_server_stack_info_cmd, // fetch server stack info 
            apply_server_stack_update, // apply server stack update
            paas_init_project, // paas init project
            paas_scan_project, // paas scan project
            paas_project_build, // paas project build
            paas_project_test, // paas project test
            paas_test_local, // paas test local
            paas_apply_gen, // paas apply gen
            paas_pipeline, // paas pipeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
