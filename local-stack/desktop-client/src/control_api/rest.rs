//! control-api REST endpoints (projects, nginx, storage, host DB, deploy HTTP fallback).

use deploy_client::DeployProgressEvent;
use deploy_core::pirate_project::PirateManifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::auth;
use crate::connection::{
    clear_control_api_jwt, load_control_api_base, load_control_api_direct_url,
};
use futures_util::future::join_all;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Matches control-api JSON for `POST .../host-services/:id/install|remove` (server `HostServiceActionView`).
#[derive(Deserialize)]
struct HostServiceActionBody {
    ok: bool,
    message: String,
    #[serde(default)]
    output: Option<String>,
}

#[derive(Deserialize)]
struct ControlApiErrorPayload {
    error: ControlApiErrorInner,
}

#[derive(Deserialize)]
struct ControlApiErrorInner {
    code: String,
    message: String,
}

/// Prefer `error.message` from control-api JSON bodies.
pub fn format_control_api_http_error(
    status: reqwest::StatusCode,
    body: &str,
    action: &str,
) -> String {
    if let Ok(v) = serde_json::from_str::<ControlApiErrorPayload>(body) {
        let msg = v.error.message.trim();
        if !msg.is_empty() {
            return format!("{} (HTTP {} / {})", msg, status.as_u16(), v.error.code);
        }
    }
    let snippet = body.chars().take(400).collect::<String>();
    format!("{action} HTTP {}: {snippet}", status.as_u16())
}

fn ensure_host_service_action_ok(text: &str) -> Result<(), String> {
    let v: HostServiceActionBody = serde_json::from_str(text).map_err(|e| {
        format!(
            "invalid host-service JSON: {e}: {}",
            text.chars().take(240).collect::<String>()
        )
    })?;
    if v.ok {
        return Ok(());
    }
    let detail = v.output.as_deref().unwrap_or("").trim();
    if detail.is_empty() {
        Err(v.message)
    } else {
        Err(format!("{}: {}", v.message, detail))
    }
}

#[derive(Deserialize)]
struct ProjectsResponse {
    projects: Vec<ProjectEntry>,
}

#[derive(Deserialize)]
struct ProjectEntry {
    id: String,
    deploy_root: String,
}

#[derive(Deserialize)]
struct StatusResponse {
    current_version: String,
    state: String,
    source: String,
    #[serde(default)]
    max_upload_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectRow {
    pub id: String,
    pub deploy_root: String,
    pub state: String,
    pub current_version: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_upload_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectsOverview {
    pub projects: Vec<ServerProjectRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) fn normalize_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

/// Human-readable size for deploy limit messages: `N bytes (X.XX MiB)`.
pub fn format_bytes_with_mib(n: u64) -> String {
    let mib = n as f64 / (1024.0 * 1024.0);
    format!("{n} bytes ({mib:.2} MiB)")
}

fn artifact_limit_preflight_error(artifact_bytes: u64, limit: u64) -> String {
    format!(
        "packed artifact {} exceeds HTTP deploy limit {} (effective max from GET /api/v1/status). Set DEPLOY_MAX_UPLOAD_BYTES on the server for deploy-server and control-api to the same value (or higher) and restart both services.",
        format_bytes_with_mib(artifact_bytes),
        format_bytes_with_mib(limit),
    )
}

pub(crate) fn health_probe_summary(base: &str) -> String {
    let health_url = format!("{}/health", base);
    let client = match crate::http_client::blocking_client() {
        Ok(c) => c,
        Err(_) => return "health=client_build_failed".to_string(),
    };
    match client.get(&health_url).send() {
        Ok(resp) => format!("health_http={}", resp.status()),
        Err(e) => {
            if e.is_timeout() {
                "health=timeout".to_string()
            } else if e.is_connect() {
                "health=connect_error".to_string()
            } else {
                format!("health_error={e}")
            }
        }
    }
}

pub(crate) fn fmt_reqwest_send_err(e: reqwest::Error, url: &str) -> String {
    let mut s = format!("{e}");
    if let Some(src) = std::error::Error::source(&e) {
        s.push_str(": ");
        s.push_str(&src.to_string());
    }
    let el = s.to_ascii_lowercase();
    if el.contains("receiver is gone")
        || el.contains("connection reset")
        || el.contains("broken pipe")
    {
        s.push_str(
            " — the HTTP connection closed while sending or reading the body (unstable network, proxy/nginx timeout, or server closed the connection). Smaller chunks or upgrading to resumable upload (…/deploy-artifact/session) avoids losing the whole transfer.",
        );
    } else if e.is_connect() {
        s.push_str(
            " — cannot reach control-api at this URL (wrong port/host, firewall, or use the HTTP base nginx exposes; set DEPLOY_CONTROL_API_PUBLIC_URL on deploy-server so GetStatus advertises the right base).",
        );
    } else if e.is_timeout() {
        s.push_str(
            " — request timed out (increase proxy/client timeouts for very large artifacts).",
        );
    }
    if !s.contains(url) {
        s.push_str(&format!(" (url: {url})"));
    }
    s
}

async fn fetch_status_async(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    project_id: &str,
) -> Result<StatusResponse, String> {
    let url = format!("{}/api/v1/status", base);
    let resp = client
        .get(&url)
        .query(&[("project", project_id)])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "HTTP {}: {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// GET `/api/v1/projects` and parallel GET `/api/v1/status` per project (chunks of 6).
pub fn fetch_server_projects_overview() -> Result<ServerProjectsOverview, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;

    crate::tokio_runtime::block_on(async move {
        let client = crate::http_client::async_client()?.clone();

        let projects_url = format!("{}/api/v1/projects", base);
        let presp = client
            .get(&projects_url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let pstatus = presp.status();
        if pstatus == reqwest::StatusCode::UNAUTHORIZED {
            let _ = clear_control_api_jwt();
            return Err("control-api returned 401; sign in again".into());
        }
        if !pstatus.is_success() {
            let body = presp.text().await.unwrap_or_default();
            return Err(format!(
                "projects HTTP {}: {}",
                pstatus,
                body.chars().take(200).collect::<String>()
            ));
        }

        let plist: ProjectsResponse = presp.json().await.map_err(|e| e.to_string())?;

        let mut rows: Vec<ServerProjectRow> = Vec::new();
        const CHUNK: usize = 6;

        for chunk in plist.projects.chunks(CHUNK) {
            let mut futs = Vec::new();
            for pv in chunk {
                let pid = pv.id.clone();
                let dr = pv.deploy_root.clone();
                let b = base.clone();
                let t = token.clone();
                let cl = client.clone();
                futs.push(async move {
                    match fetch_status_async(&cl, &b, &t, &pid).await {
                        Ok(s) => ServerProjectRow {
                            id: pid,
                            deploy_root: dr,
                            state: s.state,
                            current_version: s.current_version,
                            source: s.source,
                            max_upload_bytes: s.max_upload_bytes,
                            status_error: None,
                        },
                        Err(e) => ServerProjectRow {
                            id: pid,
                            deploy_root: dr,
                            state: "—".into(),
                            current_version: "—".into(),
                            source: "—".into(),
                            max_upload_bytes: None,
                            status_error: Some(e),
                        },
                    }
                });
            }
            let chunk_rows = join_all(futs).await;
            rows.extend(chunk_rows);
        }

        Ok(ServerProjectsOverview {
            projects: rows,
            error: None,
        })
    })
}

#[derive(Deserialize)]
struct AllocateProjectIdResponse {
    id: String,
}

/// POST `/api/v1/projects/allocate` — creates a new deploy slot on the server (directory + optional DB).
pub fn allocate_remote_project_id() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/projects/allocate", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({}))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "allocate HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    let r: AllocateProjectIdResponse = resp.json().map_err(|e| e.to_string())?;
    let id = r.id.trim();
    if id.is_empty() {
        return Err("server returned empty project id".into());
    }
    deploy_core::validate_project_id(id).map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

/// Sets `[project].deploy_project_id` and rewrites `pirate.toml` (TOML round-trip; comments may be lost).
pub fn write_pirate_toml_deploy_project_id(project_root: &Path, id: &str) -> Result<(), String> {
    deploy_core::validate_project_id(id).map_err(|e| e.to_string())?;
    let path = project_root.join("pirate.toml");
    let mut m = PirateManifest::read_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    m.project.deploy_project_id = id.trim().to_string();
    let s = m
        .to_toml_string()
        .map_err(|e| format!("serialize pirate.toml: {e}"))?;
    std::fs::write(&path, s).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Allocates id on server, saves to `pirate.toml`, updates active gRPC project in SQLite.
pub fn allocate_and_apply_remote_project_id(
    project_dir: std::path::PathBuf,
) -> Result<String, String> {
    let id = allocate_remote_project_id()?;
    write_pirate_toml_deploy_project_id(&project_dir, &id)?;
    crate::connection::set_active_project(id.clone())?;
    Ok(id)
}

/// Before deploy/pipeline/rollback: resolve gRPC project id — reuse explicit non-`default`
/// `[project].deploy_project_id` in `pirate.toml`, or keep `default` if that slot already has a real app
/// release (`GetStatus` on `default`: non-empty `current_version` not starting with `stack@`), otherwise
/// allocate a new slot via control-api (requires JWT).
pub fn ensure_deploy_project_id_for_deploy(
    project_dir: std::path::PathBuf,
) -> Result<String, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let raw = m.project.deploy_project_id.trim();
    if !raw.is_empty() && !raw.eq_ignore_ascii_case("default") {
        let target = m.project.deploy_target_project_id();
        deploy_core::validate_project_id(&target).map_err(|e| e.to_string())?;
        crate::connection::set_active_project(target.clone())?;
        return Ok(target);
    }

    let status = crate::connection::verify_grpc_status_for_project("default")?;
    let cv = status.current_version.trim();
    let needs_new_slot = cv.is_empty() || cv.starts_with("stack@");

    if !needs_new_slot {
        crate::connection::set_active_project("default".to_string())?;
        return Ok("default".to_string());
    }

    allocate_and_apply_remote_project_id(project_dir)
}

fn project_param(project_id: &str) -> Result<String, String> {
    let raw = project_id.trim();
    let pid = if raw.is_empty() {
        "default".to_string()
    } else {
        deploy_core::normalize_project_id(raw)
    };
    deploy_core::validate_project_id(&pid).map_err(|e| e.to_string())?;
    Ok(pid)
}

/// `GET /api/v1/status?project=…` — JSON body as string (for dashboard tools).
pub fn control_api_fetch_status_json(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/status", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .query(&[("project", pid.as_str())])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "status HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/projects/telemetry?project=…&logs_limit=…` — JSON body as string.
pub fn control_api_fetch_project_telemetry_json(
    project_id: &str,
    logs_limit: usize,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/projects/telemetry", base);
    let logs_limit_s = logs_limit.max(1).to_string();
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .query(&[
            ("project", pid.as_str()),
            ("logs_limit", logs_limit_s.as_str()),
        ])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "project telemetry HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/projects/telemetry/clear?project=…` — truncates `.pirate/runtime.log` on the host.
pub fn control_api_clear_project_runtime_log(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/projects/telemetry/clear", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .query(&[("project", pid.as_str())])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "telemetry clear HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/app-env?project=…` — JSON string.
pub fn control_api_fetch_app_env_json(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/app-env", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .query(&[("project", pid.as_str())])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "app-env HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/app-env?project=…` with `{"content":…}`.
pub fn control_api_put_app_env(project_id: &str, content: &str) -> Result<(), String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/app-env", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .query(&[("project", pid.as_str())])
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "content": content }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "app-env PUT HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    Ok(())
}

/// `GET /api/v1/host-deploy-env` — JSON (`HostDeployEnvView`).
pub fn control_api_fetch_host_deploy_env_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-deploy-env", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-deploy-env HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/host-deploy-env` with `{"content":…}` — schedules systemd restarts on the server.
pub fn control_api_put_host_deploy_env(content: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-deploy-env", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "content": content }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-deploy-env PUT HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/host-deploy-env/template` — JSON with `template` string (reference env.example).
pub fn control_api_fetch_host_deploy_env_template_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-deploy-env/template", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-deploy-env template HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/nginx/status` — JSON (`NginxStatusView`).
pub fn control_api_fetch_nginx_status_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/nginx/status", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx status HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/host-services` — JSON (`HostServicesView`).
pub fn control_api_fetch_host_services_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-services", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-services HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/host-services/{id}/install` with JSON body `{ "env": { "KEY": "value" } }`.
/// Pass `install_env_json` as JSON object string, e.g. `{"env":{}}` or `{"env":{"PIRATE_…":"…"}}`.
pub fn control_api_host_service_install(
    id: &str,
    install_env_json: Option<&str>,
) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("service id is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-services/{}/install", base, id);
    let body: serde_json::Value = match install_env_json {
        None => serde_json::json!({ "env": {} }),
        Some(s) if s.trim().is_empty() => serde_json::json!({ "env": {} }),
        Some(s) => {
            serde_json::from_str(s).map_err(|e| format!("install env must be valid JSON: {e}"))?
        }
    };
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-service install HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    ensure_host_service_action_ok(&text)?;
    Ok(text)
}

/// `POST /api/v1/host-services/{id}/remove`
pub fn control_api_host_service_remove(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("service id is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-services/{}/remove", base, id);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-service remove HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    ensure_host_service_action_ok(&text)?;
    Ok(text)
}

/// `GET /api/v1/host-services/{id}/runtime-config` — JSON `HostServiceRuntimeConfigView` (minio, meilisearch).
pub fn control_api_host_service_runtime_get_json(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("service id is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-services/{}/runtime-config", base, id);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-service runtime GET HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/host-services/{id}/runtime-config` — body JSON `{"env":{...}}`.
pub fn control_api_host_service_runtime_put_json(
    id: &str,
    body_json: &str,
) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("service id is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-services/{}/runtime-config", base, id);
    let v: serde_json::Value = serde_json::from_str(body_json)
        .map_err(|e| format!("runtime config body must be valid JSON: {e}"))?;
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&v)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-service runtime PUT HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    ensure_host_service_action_ok(&text)?;
    Ok(text)
}

/// `POST /api/v1/host-services/{id}/restart` (minio, meilisearch).
pub fn control_api_host_service_restart(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("service id is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-services/{}/restart", base, id);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-service restart HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    ensure_host_service_action_ok(&text)?;
    Ok(text)
}

/// `GET /api/v1/nginx/site` — JSON (`NginxConfigView`).
pub fn control_api_fetch_nginx_site_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/nginx/site", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx site HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/nginx/site` with `{"content":...}` — validate + reload via helper.
pub fn control_api_put_nginx_site(content: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/nginx/site", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "content": content }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx site PUT HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/nginx/file?path=...` — JSON (`NginxConfigView`).
pub fn control_api_fetch_nginx_file_json(path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("nginx file path is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/nginx/file?path={}",
        base,
        urlencoding::encode(path)
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx file GET HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/nginx/file` — `path` + `content` (privileged apply on server).
pub fn control_api_put_nginx_file_json(path: &str, content: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("nginx file path is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/nginx/file", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "path": path, "content": content }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx file PUT HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

fn direct_url_likely_loopback_for_remote_client(url: &str) -> bool {
    let t = url.to_ascii_lowercase();
    t.contains("127.0.0.1")
        || t.contains("localhost")
        || t.contains("[::1]")
        || t.contains("0.0.0.0")
}

/// `POST /api/v1/nginx/ensure` with mode (`api_only` | `with_ui`).
///
/// If the primary control-api base (often HTTPS via nginx) is not reachable yet, retries once
/// with [`load_control_api_direct_url`] from the last gRPC `GetStatus` (e.g. `http://host:8080`).
pub fn control_api_ensure_nginx(mode: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/nginx/ensure", base);
    let client = crate::http_client::blocking_client()?;
    let resp = match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "mode": mode }))
        .send()
    {
        Ok(r) => r,
        Err(e) if e.is_connect() => {
            let Some(fb) = load_control_api_direct_url() else {
                return Err(fmt_reqwest_send_err(e, &url));
            };
            let fb = normalize_base(&fb);
            if fb == base {
                return Err(fmt_reqwest_send_err(e, &url));
            }
            if direct_url_likely_loopback_for_remote_client(&fb) {
                let mut m = fmt_reqwest_send_err(e, &url);
                m.push_str(" — GetStatus direct control-api URL is loopback; from another machine set `DEPLOY_CONTROL_API_DIRECT_URL=http://<this-server-lan-or-public-ip>:8080` on deploy-server, reconnect gRPC, then try again (or use SSH port-forward to :8080).");
                return Err(m);
            }
            let url2 = format!("{}/api/v1/nginx/ensure", fb);
            client
                .post(&url2)
                .header("Authorization", format!("Bearer {}", token))
                .json(&serde_json::json!({ "mode": mode }))
                .send()
                .map_err(|e2| {
                    let mut m = format!(
                        "primary base failed ({}), retried with GetStatus direct URL {}: {}",
                        fmt_reqwest_send_err(e, &url),
                        fb,
                        fmt_reqwest_send_err(e2, &url2)
                    );
                    m.push_str(" — if both fail, check firewall (8080) and that control-api binds to 0.0.0.0, or temporarily set the HTTP base in the app to the direct URL (http://<host>:8080).");
                    m
                })?
        }
        Err(e) => return Err(fmt_reqwest_send_err(e, &url)),
    };
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx ensure HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/projects/:id/nginx/apply` — write per-project vhost on the host from `pirate.toml`.
pub fn control_api_apply_project_nginx(project_dir: &Path) -> Result<String, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let manifest_toml = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let project_id = m.project.deploy_target_project_id();
    deploy_core::validate_project_id(&project_id).map_err(|e| e.to_string())?;

    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/projects/{}/nginx/apply", base, project_id);
    let body = serde_json::json!({ "manifest_toml": manifest_toml });
    let client = crate::http_client::blocking_client()?;
    let resp = match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
    {
        Ok(r) => r,
        Err(e) if e.is_connect() => {
            let Some(fb) = load_control_api_direct_url() else {
                return Err(fmt_reqwest_send_err(e, &url));
            };
            let fb = normalize_base(&fb);
            if fb == base {
                return Err(fmt_reqwest_send_err(e, &url));
            }
            if direct_url_likely_loopback_for_remote_client(&fb) {
                let mut m = fmt_reqwest_send_err(e, &url);
                m.push_str(" — GetStatus direct control-api URL is loopback; set DEPLOY_CONTROL_API_DIRECT_URL on the server or use SSH port-forward to :8080.");
                return Err(m);
            }
            let url2 = format!("{}/api/v1/projects/{}/nginx/apply", fb, project_id);
            client
                .post(&url2)
                .header("Authorization", format!("Bearer {}", token))
                .json(&body)
                .send()
                .map_err(|e2| {
                    format!(
                        "primary base failed ({}), retried with GetStatus direct URL {}: {}",
                        fmt_reqwest_send_err(e, &url),
                        fb,
                        fmt_reqwest_send_err(e2, &url2)
                    )
                })?
        }
        Err(e) => return Err(fmt_reqwest_send_err(e, &url)),
    };
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "project nginx apply HTTP {}: {}",
            status,
            text.chars().take(400).collect::<String>()
        ));
    }
    #[derive(Deserialize)]
    struct ApplyView {
        ok: bool,
        path: String,
        message: String,
        #[serde(default)]
        warnings: Vec<String>,
    }
    let v: ApplyView =
        serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}: {text}"))?;
    if !v.ok {
        return Err(if v.message.is_empty() {
            format!("project nginx apply failed at {}", v.path)
        } else {
            format!("{} (path: {})", v.message, v.path)
        });
    }
    let mut msg = format!("OK: {} — {}", v.path, v.message);
    if !v.warnings.is_empty() {
        msg.push_str("\nWarnings: ");
        msg.push_str(&v.warnings.join("; "));
    }
    Ok(msg)
}

/// `GET /api/v1/nginx/sites` — full inventory JSON (`NginxSitesView`).
pub fn control_api_fetch_nginx_sites_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/nginx/sites", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx sites HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/nginx/preflight` — JSON body `NginxPreflightProposed`, response `NginxPreflightView`.
pub fn control_api_nginx_preflight_json(body: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let body_json: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("preflight body JSON: {e}"))?;
    let url = format!("{}/api/v1/nginx/preflight", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body_json)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx preflight HTTP {}: {}",
            status,
            t.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/nginx/action` — JSON `NginxActionBody`, response `NginxActionResponseView`.
pub fn control_api_nginx_action_json(body: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let body_json: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("nginx action body JSON: {e}"))?;
    let url = format!("{}/api/v1/nginx/action", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body_json)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "nginx action HTTP {}: {}",
            status,
            t.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/process/restart?project=…` — JSON string (`ProcessControlView`).
pub fn control_api_restart_process_json(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/process/restart", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .query(&[("project", pid.as_str())])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format_control_api_http_error(status, &body, "restart"));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/process/stop?project=…` — JSON string (`ProcessControlView`).
pub fn control_api_stop_process_json(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/process/stop", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .query(&[("project", pid.as_str())])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format_control_api_http_error(status, &body, "stop"));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/process/listeners?project=…&scope=project|all`
pub fn control_api_fetch_process_listeners_json(
    project_id: &str,
    scope: &str,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_param(project_id)?;
    let sc = {
        let s = scope.trim();
        if s.is_empty() {
            "project"
        } else {
            s
        }
    };
    let url = format!("{}/api/v1/process/listeners", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .query(&[("project", pid.as_str()), ("scope", sc)])
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format_control_api_http_error(status, &body, "listeners"));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/process/kill-listener` — optional `root_password`, `allow_foreign`.
pub fn control_api_kill_process_listener_json(
    project_id: &str,
    pid: u32,
    signal: &str,
    port: Option<u16>,
    root_password: Option<&str>,
    allow_foreign: bool,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid_proj = project_param(project_id)?;
    let url = format!("{}/api/v1/process/kill-listener", base);
    let sig = {
        let s = signal.trim();
        if s.is_empty() {
            "TERM"
        } else {
            s
        }
    };
    let body = serde_json::json!({
        "project": pid_proj,
        "pid": pid,
        "signal": sig,
        "port": port,
        "root_password": root_password,
        "allow_foreign": allow_foreign,
    });
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format_control_api_http_error(status, &body, "kill"));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/antiddos` — JSON (`AntiddosGetResponse`).
pub fn control_api_antiddos_get_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/antiddos", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "antiddos GET HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/antiddos` — full JSON body.
pub fn control_api_antiddos_put_json(content: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/antiddos", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(content.to_string())
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "antiddos PUT HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

fn antiddos_post(path: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "antiddos POST HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/antiddos/enable`
pub fn control_api_antiddos_enable() -> Result<String, String> {
    antiddos_post("api/v1/antiddos/enable")
}

/// `POST /api/v1/antiddos/disable`
pub fn control_api_antiddos_disable() -> Result<String, String> {
    antiddos_post("api/v1/antiddos/disable")
}

/// `POST /api/v1/antiddos/apply`
pub fn control_api_antiddos_apply() -> Result<String, String> {
    antiddos_post("api/v1/antiddos/apply")
}

/// `GET /api/v1/antiddos/stats`
pub fn control_api_antiddos_stats_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/antiddos/stats", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "antiddos stats HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `PUT /api/v1/antiddos/projects/:project_id`
pub fn control_api_antiddos_project_put_json(
    project_id: &str,
    content: &str,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }
    let url = format!(
        "{}/api/v1/antiddos/projects/{}",
        base,
        urlencoding::encode(pid)
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(content.to_string())
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "antiddos project PUT HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `DELETE /api/v1/antiddos/projects/:project_id`
pub fn control_api_antiddos_project_delete(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }
    let url = format!(
        "{}/api/v1/antiddos/projects/{}",
        base,
        urlencoding::encode(pid)
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "antiddos project DELETE HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
struct DeployArtifactMultipartJson {
    status: String,
    deployed_version: String,
}

#[derive(Debug, Serialize)]
struct DeployUploadSessionCreateBody {
    version: String,
    manifest_toml: Option<String>,
    artifact_bytes: u64,
    artifact_sha256: String,
}

#[derive(Debug, Deserialize)]
struct DeployUploadSessionCreateOut {
    upload_id: String,
    chunk_bytes: usize,
    received_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct DeployUploadSessionChunkOut {
    received_bytes: u64,
}

/// Result of `POST /api/v1/projects/:project_id/deploy-artifact` (multipart `.tar.gz`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployArtifactMultipartResult {
    pub status: String,
    pub deployed_version: String,
    /// When true, the resumable chunked session API was used; when false, legacy single multipart POST.
    #[serde(default)]
    pub used_chunked_session: bool,
}

fn is_retryable_upload_error(e: &str) -> bool {
    let s = e.to_ascii_lowercase();
    [
        "timed out",
        "timeout",
        "receiver is gone",
        "connection reset",
        "broken pipe",
        "channel closed",
        "connect error",
        "connection closed",
        "http 502",
        "http 503",
        "http 504",
        "http 408",
        "http 429",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}

async fn control_api_deploy_artifact_multipart_legacy<F>(
    base_url: &str,
    project_id: &str,
    version: &str,
    artifact_path: &Path,
    manifest_toml: Option<&str>,
    bearer_token: &str,
    artifact_bytes: u64,
    on_event: &Arc<Mutex<F>>,
) -> Result<DeployArtifactMultipartResult, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let emit = |ev: DeployProgressEvent| {
        if let Ok(mut f) = on_event.lock() {
            (f)(ev);
        }
    };

    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }
    let ver = version.trim();
    if ver.is_empty() {
        return Err("version is empty".into());
    }

    emit(DeployProgressEvent::phase_only("upload"));
    emit(DeployProgressEvent::upload_bytes(0, artifact_bytes));

    let url = format!(
        "{}/api/v1/projects/{}/deploy-artifact",
        base,
        urlencoding::encode(pid)
    );
    let path = artifact_path.to_path_buf();
    let version_owned = ver.to_string();
    let manifest = manifest_toml.map(|s| s.to_string());
    let token = bearer_token.trim().to_string();

    let result: Result<DeployArtifactMultipartJson, String> =
        tokio::task::spawn_blocking(move || {
            let client = crate::http_client::blocking_client_upload()?;

            let artifact_part = reqwest::blocking::multipart::Part::file(&path)
                .map_err(|e| format!("multipart artifact file: {e}"))?
                .file_name("artifact.tar.gz");

            let mut form = reqwest::blocking::multipart::Form::new().text("version", version_owned);
            if let Some(m) = manifest {
                form = form.text("manifest_toml", m);
            }
            form = form.part("artifact", artifact_part);

            let resp = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .multipart(form)
                .send()
                .map_err(|e| fmt_reqwest_send_err(e, &url))?;

            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                let _ = clear_control_api_jwt();
                return Err("control-api returned 401; sign in again".into());
            }
            if !status.is_success() {
                return Err(format!(
                    "deploy-artifact HTTP {}: {}",
                    status,
                    body.chars().take(500).collect::<String>()
                ));
            }
            serde_json::from_str::<DeployArtifactMultipartJson>(&body).map_err(|e| {
                format!(
                    "deploy-artifact JSON: {e}: {}",
                    body.chars().take(240).collect::<String>()
                )
            })
        })
        .await
        .map_err(|e| e.to_string())?;

    let out = result?;
    emit(DeployProgressEvent::upload_bytes(
        artifact_bytes,
        artifact_bytes,
    ));
    emit(DeployProgressEvent::phase_only("apply"));
    Ok(DeployArtifactMultipartResult {
        status: out.status,
        deployed_version: out.deployed_version,
        used_chunked_session: false,
    })
}

/// Above this size we refuse legacy multipart fallback when the resumable session API is missing (404).
const LEGACY_MULTIPART_FALLBACK_MAX_BYTES: u64 = 32 * 1024 * 1024;

async fn control_api_deploy_artifact_session_chunked<F>(
    base_url: &str,
    project_id: &str,
    version: &str,
    artifact_path: &Path,
    manifest_toml: Option<&str>,
    bearer_token: &str,
    artifact_bytes: u64,
    on_event: &Arc<Mutex<F>>,
) -> Result<DeployArtifactMultipartResult, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let emit = |ev: DeployProgressEvent| {
        if let Ok(mut f) = on_event.lock() {
            (f)(ev);
        }
    };

    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }
    let ver = version.trim();
    if ver.is_empty() {
        return Err("version is empty".into());
    }

    emit(DeployProgressEvent::phase_only("upload"));
    emit(DeployProgressEvent::upload_bytes_detail(
        0,
        artifact_bytes,
        "preparing resumable upload session",
    ));

    let session_create_url = format!(
        "{}/api/v1/projects/{}/deploy-artifact/session",
        base,
        urlencoding::encode(pid)
    );
    let session_base_url = session_create_url.clone();
    let path = artifact_path.to_path_buf();
    let version_owned = ver.to_string();
    let manifest = manifest_toml.map(|s| s.to_string());
    let token = bearer_token.trim().to_string();
    let on_event_blocking = Arc::clone(on_event);

    let result: Result<DeployArtifactMultipartJson, String> = tokio::task::spawn_blocking(move || {
        let emit_blocking = |ev: DeployProgressEvent| {
            if let Ok(mut f) = on_event_blocking.lock() {
                (f)(ev);
            }
        };

        let mut file = std::fs::File::open(&path)
            .map_err(|e| format!("open artifact {}: {e}", path.display()))?;

        let mut hasher = Sha256::new();
        let mut hash_buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file
                .read(&mut hash_buf)
                .map_err(|e| format!("read artifact for sha256 {}: {e}", path.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&hash_buf[..n]);
        }
        let artifact_sha256 = format!("{:x}", hasher.finalize());
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("seek artifact {}: {e}", path.display()))?;

        let client = crate::http_client::blocking_client_upload()?;

        let create_body = DeployUploadSessionCreateBody {
            version: version_owned,
            manifest_toml: manifest,
            artifact_bytes,
            artifact_sha256,
        };

        let mut create_out: Option<DeployUploadSessionCreateOut> = None;
        let mut create_last_err: Option<String> = None;
        for attempt in 1u32..=5u32 {
            if attempt > 1 {
                emit_blocking(DeployProgressEvent::upload_bytes_detail(
                    0,
                    artifact_bytes,
                    format!("session create retry {attempt}/5"),
                ));
            }
            let send_res = client
                .post(&session_create_url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&create_body)
                .send();
            match send_res {
                Ok(create_resp) => {
                    let create_status = create_resp.status();
                    let create_body_text = create_resp.text().unwrap_or_default();
                    if create_status == reqwest::StatusCode::UNAUTHORIZED {
                        let _ = clear_control_api_jwt();
                        return Err("control-api returned 401; sign in again".into());
                    }
                    if !create_status.is_success() {
                        let err = format!(
                            "deploy-artifact session create HTTP {}: {}",
                            create_status,
                            create_body_text.chars().take(500).collect::<String>()
                        );
                        let retry_status = matches!(
                            create_status.as_u16(),
                            408 | 429 | 500 | 502 | 503 | 504
                        );
                        if attempt < 5 && (retry_status || is_retryable_upload_error(&err)) {
                            create_last_err = Some(err);
                            std::thread::sleep(Duration::from_millis(300 * attempt as u64));
                            continue;
                        }
                        return Err(err);
                    }
                    let parsed: DeployUploadSessionCreateOut =
                        serde_json::from_str(&create_body_text).map_err(|e| {
                            format!(
                                "deploy-artifact session create JSON: {e}: {}",
                                create_body_text.chars().take(240).collect::<String>()
                            )
                        })?;
                    create_out = Some(parsed);
                    break;
                }
                Err(e) => {
                    let err = fmt_reqwest_send_err(e, &session_create_url);
                    if attempt < 5 && is_retryable_upload_error(&err) {
                        create_last_err = Some(err);
                        std::thread::sleep(Duration::from_millis(300 * attempt as u64));
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        let create_out = create_out.ok_or_else(|| {
            create_last_err.unwrap_or_else(|| "deploy-artifact session create failed after retries".into())
        })?;

        let upload_id = create_out.upload_id.trim().to_string();
        if upload_id.is_empty() {
            return Err("deploy-artifact session create returned empty upload_id".into());
        }
        let sid = upload_id.chars().take(8).collect::<String>();
        emit_blocking(DeployProgressEvent::upload_bytes_detail(
            0,
            artifact_bytes,
            format!("resumable session {sid}…"),
        ));
        let chunk_bytes = if create_out.chunk_bytes == 0 {
            1024 * 1024
        } else {
            create_out.chunk_bytes
        };
        let mut offset = create_out.received_bytes;
        if offset > artifact_bytes {
            return Err(format!(
                "deploy-artifact session create returned received_bytes={} > artifact_bytes={}",
                offset, artifact_bytes
            ));
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("seek artifact to session offset {}: {e}", offset))?;
        emit_blocking(DeployProgressEvent::upload_bytes_detail(
            offset,
            artifact_bytes,
            format!("uploading (session {sid}…)",),
        ));

        let mut chunk_buf = vec![0u8; chunk_bytes];
        while offset < artifact_bytes {
            let n = file
                .read(&mut chunk_buf)
                .map_err(|e| format!("read artifact chunk at offset {}: {e}", offset))?;
            if n == 0 {
                break;
            }
            let chunk = &chunk_buf[..n];
            let chunk_sha256 = format!("{:x}", Sha256::digest(chunk));
            let chunk_url = format!(
                "{}/{}",
                session_base_url,
                urlencoding::encode(&upload_id)
            );
            let chunk_url = format!("{}{}offset={}", chunk_url, "/chunk?", offset);

            let mut last_err: Option<String> = None;
            let mut chunk_uploaded = false;
            for attempt in 1..=5 {
                if attempt > 1 {
                    emit_blocking(DeployProgressEvent::upload_bytes_detail(
                        offset,
                        artifact_bytes,
                        format!(
                            "chunk at {} B — retry {attempt}/5 (session {sid}…)",
                            offset
                        ),
                    ));
                }
                let put_resp = client
                    .put(&chunk_url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("x-chunk-sha256", &chunk_sha256)
                    .body(chunk.to_vec())
                    .send();

                match put_resp {
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().unwrap_or_default();
                        if status == reqwest::StatusCode::UNAUTHORIZED {
                            let _ = clear_control_api_jwt();
                            return Err("control-api returned 401; sign in again".into());
                        }
                        if !status.is_success() {
                            let err = format!(
                                "deploy-artifact session chunk HTTP {} at offset {}: {}",
                                status,
                                offset,
                                body.chars().take(240).collect::<String>()
                            );
                            let retry_status = matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
                            if attempt < 5 && (retry_status || is_retryable_upload_error(&err)) {
                                std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                                last_err = Some(err);
                                continue;
                            }
                            return Err(err);
                        }
                        let out: DeployUploadSessionChunkOut =
                            serde_json::from_str(&body).map_err(|e| {
                                format!(
                                    "deploy-artifact session chunk JSON: {e}: {}",
                                    body.chars().take(240).collect::<String>()
                                )
                            })?;
                        if out.received_bytes < offset {
                            return Err(format!(
                                "deploy-artifact session chunk regressed received_bytes={} < offset={}",
                                out.received_bytes, offset
                            ));
                        }
                        offset = out.received_bytes;
                        emit_blocking(DeployProgressEvent::upload_bytes(offset, artifact_bytes));
                        chunk_uploaded = true;
                        break;
                    }
                    Err(e) => {
                        let err = fmt_reqwest_send_err(e, &chunk_url);
                        if attempt < 5 && is_retryable_upload_error(&err) {
                            std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                            last_err = Some(err);
                            continue;
                        }
                        return Err(err);
                    }
                }
            }
            if !chunk_uploaded {
                return Err(last_err.unwrap_or_else(|| {
                    "deploy-artifact session chunk failed after retries".to_string()
                }));
            }
        }

        let complete_url = format!(
            "{}/{}/complete",
            session_base_url,
            urlencoding::encode(&upload_id)
        );
        emit_blocking(DeployProgressEvent::upload_bytes_detail(
            artifact_bytes,
            artifact_bytes,
            format!("finalize deploy (session {sid}…)",),
        ));
        let mut complete_parsed: Option<DeployArtifactMultipartJson> = None;
        let mut complete_last_err: Option<String> = None;
        for attempt in 1u32..=5u32 {
            if attempt > 1 {
                emit_blocking(DeployProgressEvent::upload_bytes_detail(
                    artifact_bytes,
                    artifact_bytes,
                    format!("finalize retry {attempt}/5 (session {sid}…)",),
                ));
            }
            match client
                .post(&complete_url)
                .header("Authorization", format!("Bearer {}", token))
                .send()
            {
                Ok(complete_resp) => {
                    let complete_status = complete_resp.status();
                    let complete_text = complete_resp.text().unwrap_or_default();
                    if complete_status == reqwest::StatusCode::UNAUTHORIZED {
                        let _ = clear_control_api_jwt();
                        return Err("control-api returned 401; sign in again".into());
                    }
                    if !complete_status.is_success() {
                        let err = format!(
                            "deploy-artifact session complete HTTP {}: {}",
                            complete_status,
                            complete_text.chars().take(500).collect::<String>()
                        );
                        let retry_status =
                            matches!(complete_status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
                        if attempt < 5 && (retry_status || is_retryable_upload_error(&err)) {
                            complete_last_err = Some(err);
                            std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                            continue;
                        }
                        return Err(err);
                    }
                    let parsed: DeployArtifactMultipartJson =
                        serde_json::from_str(&complete_text).map_err(|e| {
                            format!(
                                "deploy-artifact session complete JSON: {e}: {}",
                                complete_text.chars().take(240).collect::<String>()
                            )
                        })?;
                    complete_parsed = Some(parsed);
                    break;
                }
                Err(e) => {
                    let err = fmt_reqwest_send_err(e, &complete_url);
                    if attempt < 5 && is_retryable_upload_error(&err) {
                        complete_last_err = Some(err);
                        std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        complete_parsed.ok_or_else(|| {
            complete_last_err.unwrap_or_else(|| "deploy-artifact session complete failed after retries".into())
        })
    })
    .await
    .map_err(|e| e.to_string())?;

    let out = result?;
    emit(DeployProgressEvent::upload_bytes(
        artifact_bytes,
        artifact_bytes,
    ));
    emit(DeployProgressEvent::phase_only("apply"));
    Ok(DeployArtifactMultipartResult {
        status: out.status,
        deployed_version: out.deployed_version,
        used_chunked_session: true,
    })
}

/// Multipart deploy via control-api (same effective limit as gRPC `DEPLOY_MAX_UPLOAD_BYTES` on the host).
/// Field order matches server: `version`, optional `manifest_toml`, then `artifact` file.
pub async fn control_api_deploy_artifact_multipart<F>(
    base_url: &str,
    project_id: &str,
    version: &str,
    artifact_path: &Path,
    manifest_toml: Option<&str>,
    bearer_token: &str,
    artifact_bytes: u64,
    on_event: &Arc<Mutex<F>>,
) -> Result<DeployArtifactMultipartResult, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }

    let client = crate::http_client::async_client()?.clone();
    match fetch_status_async(&client, &base, bearer_token.trim(), pid).await {
        Ok(st) => {
            if let Some(limit) = st.max_upload_bytes {
                if artifact_bytes > limit {
                    return Err(artifact_limit_preflight_error(artifact_bytes, limit));
                }
            }
        }
        Err(_) => {
            // GET /api/v1/status failed; session create will still enforce limits.
        }
    }

    match control_api_deploy_artifact_session_chunked(
        base_url,
        project_id,
        version,
        artifact_path,
        manifest_toml,
        bearer_token,
        artifact_bytes,
        on_event,
    )
    .await
    {
        Ok(out) => Ok(out),
        Err(e) => {
            let le = e.to_ascii_lowercase();
            let route_missing = le.contains("404")
                || le.contains("not found")
                || le.contains("unknown upload session")
                || le.contains("missing route");
            if route_missing && artifact_bytes > LEGACY_MULTIPART_FALLBACK_MAX_BYTES {
                return Err(format!(
                    "{e} — For artifacts larger than {} MiB, resumable upload (POST …/deploy-artifact/session) is required. Upgrade control-api and ensure nginx proxies `/api/` to it; legacy single-request multipart is disabled for this size because it usually fails on large or unstable transfers.",
                    LEGACY_MULTIPART_FALLBACK_MAX_BYTES / (1024 * 1024)
                ));
            }
            if route_missing {
                control_api_deploy_artifact_multipart_legacy(
                    base_url,
                    project_id,
                    version,
                    artifact_path,
                    manifest_toml,
                    bearer_token,
                    artifact_bytes,
                    on_event,
                )
                .await
            } else {
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pirate file storage (control-api `/api/v1/storage/*`, JWT)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntryView {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub mtime_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageListView {
    pub path: String,
    pub entries: Vec<StorageEntryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageUsageView {
    pub used_bytes: u64,
    pub max_bytes: u64,
    #[serde(default)]
    pub free_bytes: Option<u64>,
    #[serde(default)]
    pub used_percent: Option<f32>,
}

/// `GET /api/v1/storage/tree?path=`
pub fn control_api_storage_tree_json(path: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let path_q = path.trim();
    let url = format!(
        "{}/api/v1/storage/tree?path={}",
        base,
        urlencoding::encode(path_q)
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage tree HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/storage/usage` — returns JSON
pub fn control_api_storage_usage_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/usage", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage usage HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/storage/folders` with `{"path":"a/b"}`.
pub fn control_api_storage_create_folder(path: &str) -> Result<(), String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/folders", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "path": path.trim() }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage create folder HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    Ok(())
}

/// `POST /api/v1/storage/delete-folder` (JSON) — some nginx setups block `DELETE`; same semantics as
/// `DELETE /api/v1/storage/folders?…`.
pub fn control_api_storage_delete_folder(path: &str, recursive: bool) -> Result<(), String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/delete-folder", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "path": path.trim(), "recursive": recursive }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage delete folder HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    Ok(())
}

/// `POST /api/v1/storage/delete-file` (JSON) — `DELETE …/files` is blocked on some HTTP paths.
pub fn control_api_storage_delete_file(path: &str) -> Result<(), String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/delete-file", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "path": path.trim() }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage delete file HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    Ok(())
}

/// `POST /api/v1/storage/rename` — `{"from","to"}`; `PATCH` is blocked for some users behind nginx.
pub fn control_api_storage_rename(from: &str, to: &str) -> Result<(), String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{base}/api/v1/storage/rename");
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "from": from.trim(), "to": to.trim() }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage rename HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    Ok(())
}

/// Files larger than this use `POST /api/v1/storage/upload-sessions` (resumable, same as deploy).
const STORAGE_RESUMABLE_MIN_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize)]
struct StorageUploadSessionCreateBody {
    path: String,
    file_bytes: u64,
    file_sha256: String,
}

#[derive(Deserialize)]
struct StorageUploadSessionCreateOut {
    upload_id: String,
    chunk_bytes: usize,
    #[serde(default)]
    received_bytes: u64,
}

#[derive(Deserialize)]
struct StorageUploadSessionChunkOut {
    received_bytes: u64,
}

/// Resumable storage upload: session create → PUT chunks → complete (retries on transient errors).
fn control_api_storage_upload_resumable(
    local_path: &Path,
    remote_rel: &str,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let session_base_url = format!("{}/api/v1/storage/upload-sessions", base);
    let session_create_url = session_base_url.clone();

    let path = local_path.to_path_buf();
    let rel_owned = remote_rel.trim().to_string();

    let mut file = std::fs::File::open(&path)
        .map_err(|e| format!("open local file {}: {e}", path.display()))?;
    let artifact_bytes = file
        .metadata()
        .map_err(|e| format!("metadata {}: {e}", path.display()))?
        .len();
    let mut hasher = Sha256::new();
    let mut hash_buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut hash_buf)
            .map_err(|e| format!("read file for sha256 {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&hash_buf[..n]);
    }
    let file_sha256 = format!("{:x}", hasher.finalize());
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("seek {}: {e}", path.display()))?;

    let client = crate::http_client::blocking_client_upload()?;

    let create_body = StorageUploadSessionCreateBody {
        path: rel_owned,
        file_bytes: artifact_bytes,
        file_sha256,
    };

    let create_out: StorageUploadSessionCreateOut = (|| {
        let mut create_last_err: Option<String> = None;
        for attempt in 1u32..=5u32 {
            match client
                .post(&session_create_url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&create_body)
                .send()
            {
                Ok(create_resp) => {
                    let st = create_resp.status();
                    let create_body_text = create_resp.text().unwrap_or_default();
                    if st == reqwest::StatusCode::UNAUTHORIZED {
                        let _ = clear_control_api_jwt();
                        return Err("control-api returned 401; sign in again".into());
                    }
                    if st == reqwest::StatusCode::NOT_FOUND {
                        return Err("storage resumable upload API (POST /api/v1/storage/upload-sessions) is not available; upgrade control-api, or set a lower max upload / split the file".into());
                    }
                    if !st.is_success() {
                        let err = format!(
                            "storage upload-sessions create HTTP {}: {}",
                            st,
                            create_body_text.chars().take(500).collect::<String>()
                        );
                        let retry = matches!(st.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
                        if attempt < 5 && (retry || is_retryable_upload_error(&err)) {
                            create_last_err = Some(err);
                            std::thread::sleep(Duration::from_millis(300 * attempt as u64));
                            continue;
                        }
                        return Err(err);
                    }
                    return serde_json::from_str(&create_body_text).map_err(|e| {
                        format!(
                            "storage session create JSON: {e}: {}",
                            create_body_text.chars().take(240).collect::<String>()
                        )
                    });
                }
                Err(e) => {
                    let err = fmt_reqwest_send_err(e, &session_create_url);
                    if attempt < 5 && is_retryable_upload_error(&err) {
                        create_last_err = Some(err);
                        std::thread::sleep(Duration::from_millis(300 * attempt as u64));
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        Err(create_last_err.unwrap_or_else(|| "storage session create failed after retries".into()))
    })()?;

    let upload_id = create_out.upload_id.trim().to_string();
    if upload_id.is_empty() {
        return Err("storage session create returned empty upload_id".into());
    }
    let chunk_bytes = if create_out.chunk_bytes == 0 {
        1024 * 1024
    } else {
        create_out.chunk_bytes
    };
    let mut offset = create_out.received_bytes;
    if offset > artifact_bytes {
        return Err(format!(
            "storage session create: received_bytes={} > file size {}",
            offset, artifact_bytes
        ));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("seek to offset {offset}: {e}"))?;

    let mut chunk_buf = vec![0u8; chunk_bytes];
    while offset < artifact_bytes {
        let n = file
            .read(&mut chunk_buf)
            .map_err(|e| format!("read chunk at offset {offset}: {e}"))?;
        if n == 0 {
            break;
        }
        let chunk = &chunk_buf[..n];
        let chunk_sha = format!("{:x}", Sha256::digest(chunk));
        let chunk_path = format!("{}/{}", session_base_url, urlencoding::encode(&upload_id));
        let chunk_url = format!("{}/chunk?offset={}", chunk_path, offset);
        let mut last_err: Option<String> = None;
        let mut done = false;
        for attempt in 1..=5u32 {
            match client
                .put(&chunk_url)
                .header("Authorization", format!("Bearer {}", token))
                .header("x-chunk-sha256", &chunk_sha)
                .body(chunk.to_vec())
                .send()
            {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().unwrap_or_default();
                    if status == reqwest::StatusCode::UNAUTHORIZED {
                        let _ = clear_control_api_jwt();
                        return Err("control-api returned 401; sign in again".into());
                    }
                    if !status.is_success() {
                        let err = format!(
                            "storage session chunk HTTP {} at offset {}: {}",
                            status,
                            offset,
                            body.chars().take(240).collect::<String>()
                        );
                        let retry = matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
                        if attempt < 5 && (retry || is_retryable_upload_error(&err)) {
                            std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                            last_err = Some(err);
                            continue;
                        }
                        return Err(err);
                    }
                    let out: StorageUploadSessionChunkOut =
                        serde_json::from_str(&body).map_err(|e| {
                            format!(
                                "storage session chunk JSON: {e}: {}",
                                body.chars().take(240).collect::<String>()
                            )
                        })?;
                    if out.received_bytes < offset {
                        return Err(format!(
                            "storage session chunk regressed received_bytes={} < offset={}",
                            out.received_bytes, offset
                        ));
                    }
                    offset = out.received_bytes;
                    done = true;
                    break;
                }
                Err(e) => {
                    let err = fmt_reqwest_send_err(e, &chunk_url);
                    if attempt < 5 && is_retryable_upload_error(&err) {
                        std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                        last_err = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        if !done {
            return Err(
                last_err.unwrap_or_else(|| "storage session chunk failed after retries".into())
            );
        }
    }

    let complete_path = format!("{}/{}", session_base_url, urlencoding::encode(&upload_id));
    let complete_url = format!("{}/complete", complete_path);
    let mut complete_last: Option<String> = None;
    for attempt in 1u32..=5u32 {
        match client
            .post(&complete_url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
        {
            Ok(complete_resp) => {
                let st = complete_resp.status();
                let text = complete_resp.text().unwrap_or_default();
                if st == reqwest::StatusCode::UNAUTHORIZED {
                    let _ = clear_control_api_jwt();
                    return Err("control-api returned 401; sign in again".into());
                }
                if !st.is_success() {
                    let err = format!(
                        "storage session complete HTTP {}: {}",
                        st,
                        text.chars().take(500).collect::<String>()
                    );
                    let retry = matches!(st.as_u16(), 408 | 429 | 500 | 502 | 503 | 504);
                    if attempt < 5 && (retry || is_retryable_upload_error(&err)) {
                        complete_last = Some(err);
                        std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                        continue;
                    }
                    return Err(err);
                }
                return Ok(text);
            }
            Err(e) => {
                let err = fmt_reqwest_send_err(e, &complete_url);
                if attempt < 5 && is_retryable_upload_error(&err) {
                    complete_last = Some(err);
                    std::thread::sleep(Duration::from_millis(400 * attempt as u64));
                    continue;
                }
                return Err(err);
            }
        }
    }
    Err(complete_last.unwrap_or_else(|| "storage session complete failed after retries".into()))
}

/// Multipart: `path` = relative in storage, `file` = local path to read (streamed, no full-file RAM).
pub fn control_api_storage_upload_file(
    remote_rel: &str,
    local_file: &str,
) -> Result<String, String> {
    let p = Path::new(local_file);
    let file_bytes = std::fs::metadata(p)
        .map_err(|e| format!("stat local file {local_file}: {e}"))?
        .len();
    if file_bytes > STORAGE_RESUMABLE_MIN_BYTES {
        return control_api_storage_upload_resumable(p, remote_rel);
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/files", base);
    let f = std::fs::File::open(p).map_err(|e| format!("open {local_file}: {e}"))?;
    let len = f
        .metadata()
        .map_err(|e| format!("metadata {local_file}: {e}"))?
        .len();
    let fname: String = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("upload.bin")
        .to_string();
    let part = reqwest::blocking::multipart::Part::reader_with_length(f, len).file_name(fname);
    let form = reqwest::blocking::multipart::Form::new()
        .text("path", remote_rel.trim().to_string())
        .part("file", part);
    let client = crate::http_client::blocking_client_upload()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .multipart(form)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage upload HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/storage/files/download?path=…` — write to `local_path`.
pub fn control_api_storage_download_file(remote_rel: &str, local_path: &str) -> Result<(), String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/storage/files/download?path={}",
        base,
        urlencoding::encode(remote_rel.trim())
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage download HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    let bytes = resp.bytes().map_err(|e| e.to_string())?;
    if let Some(parent) = std::path::Path::new(local_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create_dir_all {}: {e}", parent.display()))?;
    }
    std::fs::write(local_path, &bytes).map_err(|e| format!("write {}: {e}", local_path))?;
    Ok(())
}

/// `POST /api/v1/storage/extract` — `conflict_mode`: `abort` | `overwrite` | `delete_and_overwrite`.
/// Returns response JSON (success or error body).
pub fn control_api_storage_extract_json(
    archive_path: &str,
    target_dir: Option<&str>,
    conflict_mode: &str,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/extract", base);
    let body = if let Some(td) = target_dir.filter(|s| !s.trim().is_empty()) {
        serde_json::json!({
            "archive_path": archive_path.trim(),
            "target_dir": td.trim(),
            "conflict_mode": conflict_mode,
        })
    } else {
        serde_json::json!({
            "archive_path": archive_path.trim(),
            "conflict_mode": conflict_mode,
        })
    };
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "storage extract HTTP {}: {}",
            status,
            text.chars().take(500).collect::<String>()
        ));
    }
    Ok(text)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageBindMountCandidate {
    pub mount_point: String,
    pub fstype: String,
    pub source: String,
    #[serde(default)]
    pub avail_bytes: Option<u64>,
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageBindActive {
    pub volume: String,
    pub source: String,
    pub mount_point: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageBindSourcesView {
    pub candidates: Vec<StorageBindMountCandidate>,
    pub active_binds: Vec<StorageBindActive>,
}

/// `GET /api/v1/storage/bind-sources`
pub fn control_api_storage_bind_sources_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/bind-sources", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "storage bind-sources HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/storage/bind` — JSON body; returns JSON `{ ok, message }`.
pub fn control_api_storage_bind_json(
    source_path: &str,
    volume_name: &str,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/bind", base);
    let body = serde_json::json!({
        "source_path": source_path.trim(),
        "volume_name": volume_name.trim(),
    });
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "storage bind HTTP {}: {}",
            status,
            text.chars().take(500).collect::<String>()
        ));
    }
    Ok(text)
}

/// `POST /api/v1/storage/unbind`
pub fn control_api_storage_unbind_json(volume_name: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/storage/unbind", base);
    let body = serde_json::json!({ "volume_name": volume_name.trim() });
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "storage unbind HTTP {}: {}",
            status,
            text.chars().take(500).collect::<String>()
        ));
    }
    Ok(text)
}

// --- Host databases (control-api `/api/v1/host-databases/*`) ---

const PIRATE_DB_USER_HEADER: &str = "x-pirate-db-user";
const PIRATE_DB_PASS_HEADER: &str = "x-pirate-db-password";

fn add_pirate_db_headers(
    req: reqwest::blocking::RequestBuilder,
    user: &str,
    pass: &str,
) -> reqwest::blocking::RequestBuilder {
    req.header(PIRATE_DB_USER_HEADER, user)
        .header(PIRATE_DB_PASS_HEADER, pass)
}

/// `None` — no per-request cred headers (server uses host env for DSN).
/// `Pair` — per-request DSN user/pass for this browse session or from the local `host_db_credentials.json` (encrypted).
enum HostDbCreds {
    None,
    Pair(String, String),
}

/// Resolves per-request database credentials. Empty user and password → no headers.
/// With username, password is taken from the `db_password` argument or, if empty, from the local credential store.
fn resolve_host_db_creds(
    instance_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<HostDbCreds, String> {
    let uopt = db_user.map(str::trim).filter(|s| !s.is_empty());
    let popt = db_password.map(str::trim).filter(|s| !s.is_empty());
    if uopt.is_none() && popt.is_none() {
        return Ok(HostDbCreds::None);
    }
    let Some(u) = uopt else {
        return Err(
            "set a database username when using a password or saved database credentials".into(),
        );
    };
    let u = u.to_string();
    if let Some(p) = popt {
        return Ok(HostDbCreds::Pair(u, p.to_string()));
    }
    if let Some(p) = crate::db_credentials::saved_password_plain(instance_id)? {
        if !p.is_empty() {
            return Ok(HostDbCreds::Pair(u, p));
        }
    }
    Err(
        "database password is required: enter it, or use Remember to save it in the local credential file"
            .into(),
    )
}

/// `GET /api/v1/host-databases`
pub fn control_api_host_databases_list_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v1/host-databases", base);
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-databases HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

fn enc_path(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// `GET /api/v1/host-databases/:id/schemas`
pub fn control_api_host_db_schemas_json(
    instance_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/schemas",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `GET /api/v1/host-databases/:id/tables?schema=`
pub fn control_api_host_db_tables_json(
    instance_id: &str,
    schema: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/tables?schema={}",
        base,
        enc_path(instance_id),
        urlencoding::encode(schema)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `GET .../columns/:schema/:table`
pub fn control_api_host_db_columns_json(
    instance_id: &str,
    schema: &str,
    table: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/columns/{}/{}",
        base,
        enc_path(instance_id),
        enc_path(schema),
        enc_path(table)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

#[derive(serde::Serialize)]
struct HostDbQueryBodySer<'a> {
    sql: &'a str,
    max_rows: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<&'a str>,
}

/// `POST /api/v1/host-databases/:id/query`
pub fn control_api_host_db_query_json(
    instance_id: &str,
    sql: &str,
    max_rows: u32,
    db_user: Option<&str>,
    db_password: Option<&str>,
    database: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/query",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let client = crate::http_client::blocking_client()?;
    let body = HostDbQueryBodySer {
        sql,
        max_rows,
        database: database.map(str::trim).filter(|s| !s.is_empty()),
    };
    let mut post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        post = add_pirate_db_headers(post, u, p);
    }
    let resp = post
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db query HTTP {}: {}",
            status,
            body.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET .../rows?schema=&table=&limit=&offset=`
pub fn control_api_host_db_rows_json(
    instance_id: &str,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/rows?schema={}&table={}&limit={}&offset={}",
        base,
        enc_path(instance_id),
        urlencoding::encode(schema),
        urlencoding::encode(table),
        limit,
        offset
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `GET .../relationships`
pub fn control_api_host_db_relationships_json(
    instance_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/relationships",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

fn get_json_authenticated_with_creds(
    url: &str,
    token: &str,
    creds: &HostDbCreds,
) -> Result<String, String> {
    let client = crate::http_client::blocking_client()?;
    let mut get = client
        .get(url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = *creds {
        get = add_pirate_db_headers(get, u, p);
    }
    let resp = get.send().map_err(|e| fmt_reqwest_send_err(e, url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {}: {}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET .../redis/keys?pattern=&cursor=`
pub fn control_api_host_db_redis_keys_json(
    instance_id: &str,
    pattern: &str,
    cursor: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/redis/keys?pattern={}&cursor={}",
        base,
        enc_path(instance_id),
        urlencoding::encode(pattern),
        urlencoding::encode(cursor)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `GET .../mongo/databases`
pub fn control_api_host_db_mongo_databases_json(
    instance_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/mongo/databases",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `GET .../mongo/collections?db=`
pub fn control_api_host_db_mongo_collections_json(
    instance_id: &str,
    db: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/mongo/collections?db={}",
        base,
        enc_path(instance_id),
        urlencoding::encode(db)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `GET .../mongo/preview?db=&collection=&limit=`
pub fn control_api_host_db_mongo_preview_json(
    instance_id: &str,
    db: &str,
    collection: &str,
    limit: u32,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v1/host-databases/{}/mongo/preview?db={}&collection={}&limit={}",
        base,
        enc_path(instance_id),
        urlencoding::encode(db),
        urlencoding::encode(collection),
        limit
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

// --- Host databases v2 (`/api/v2/host-databases/*`) ---

/// `GET /api/v2/host-databases/capabilities`
pub fn control_api_host_db_v2_capabilities_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!("{}/api/v2/host-databases/capabilities", base);
    get_json_authenticated_with_creds(&url, &token, &HostDbCreds::None)
}

/// `GET /api/v2/host-databases/:id/object-tree`
pub fn control_api_host_db_v2_object_tree_json(
    instance_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/object-tree",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

#[derive(serde::Serialize)]
struct HostDbV2GridBodySer {
    schema: String,
    table: String,
    limit: u32,
    offset: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_column: Option<String>,
    #[serde(default)]
    sort_desc: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter_column: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter_value: Option<serde_json::Value>,
}

/// `POST /api/v2/host-databases/:id/grid`
pub fn control_api_host_db_v2_grid_json(
    instance_id: &str,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
    sort_column: Option<&str>,
    sort_desc: bool,
    filter_column: Option<&str>,
    filter_value: Option<serde_json::Value>,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/grid",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let body = HostDbV2GridBodySer {
        schema: schema.to_string(),
        table: table.to_string(),
        limit,
        offset,
        sort_column: sort_column.map(|s| s.to_string()),
        sort_desc,
        filter_column: filter_column.map(|s| s.to_string()),
        filter_value,
    };
    let client = crate::http_client::blocking_client()?;
    let mut post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        post = add_pirate_db_headers(post, u, p);
    }
    let resp = post
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db v2 grid HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct HostDbV2RowMutateBodySer {
    op: String,
    schema: String,
    table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pk: Option<serde_json::Map<String, serde_json::Value>>,
    row: serde_json::Value,
}

/// `POST /api/v2/host-databases/:id/row-mutate`
pub fn control_api_host_db_v2_row_mutate_json(
    instance_id: &str,
    op: &str,
    schema: &str,
    table: &str,
    pk: Option<serde_json::Map<String, serde_json::Value>>,
    row: serde_json::Value,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/row-mutate",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let body = HostDbV2RowMutateBodySer {
        op: op.to_string(),
        schema: schema.to_string(),
        table: table.to_string(),
        pk,
        row,
    };
    let client = crate::http_client::blocking_client()?;
    let mut post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        post = add_pirate_db_headers(post, u, p);
    }
    let resp = post
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db v2 row-mutate HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct HostDbV2SqlJobStartSer<'a> {
    sql: &'a str,
    max_rows: u32,
}

/// `POST /api/v2/host-databases/:id/sql-jobs`
pub fn control_api_host_db_v2_sql_job_start_json(
    instance_id: &str,
    sql: &str,
    max_rows: u32,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/sql-jobs",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let body = HostDbV2SqlJobStartSer { sql, max_rows };
    let client = crate::http_client::blocking_client()?;
    let mut post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        post = add_pirate_db_headers(post, u, p);
    }
    let resp = post
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db v2 sql-job HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v2/host-databases/:id/sql-jobs/:job_id`
pub fn control_api_host_db_v2_sql_job_get_json(
    instance_id: &str,
    job_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/sql-jobs/{}",
        base,
        enc_path(instance_id),
        enc_path(job_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

/// `DELETE /api/v2/host-databases/:id/sql-jobs/:job_id`
pub fn control_api_host_db_v2_sql_job_cancel_json(
    instance_id: &str,
    job_id: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/sql-jobs/{}",
        base,
        enc_path(instance_id),
        enc_path(job_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let client = crate::http_client::blocking_client()?;
    let mut del = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        del = add_pirate_db_headers(del, u, p);
    }
    let resp = del.send().map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db v2 sql-job cancel HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v2/host-databases/:id/migration-status?database=...` (read-only tool detection).
pub fn control_api_host_db_v2_migration_status_get_json(
    instance_id: &str,
    database: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
    tools: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let mut url = format!(
        "{}/api/v2/host-databases/{}/migration-status?database={}",
        base,
        enc_path(instance_id),
        urlencoding::encode(database)
    );
    if let Some(t) = tools {
        let t = t.trim();
        if !t.is_empty() {
            url.push_str("&tools=");
            url.push_str(&urlencoding::encode(t));
        }
    }
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    get_json_authenticated_with_creds(&url, &token, &creds)
}

#[derive(serde::Serialize)]
struct HostDbV2MigrationStatusBodySer {
    database: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<String>,
}

/// `POST /api/v2/host-databases/:id/migration-status` (same as GET; useful when proxies mangle query strings).
pub fn control_api_host_db_v2_migration_status_post_json(
    instance_id: &str,
    database: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
    tools: Option<&str>,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/migration-status",
        base,
        enc_path(instance_id)
    );
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let body = HostDbV2MigrationStatusBodySer {
        database: database.to_string(),
        tools: tools
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(std::string::ToString::to_string),
    };
    let client = crate::http_client::blocking_client()?;
    let mut post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        post = add_pirate_db_headers(post, u, p);
    }
    let resp = post
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db migration-status HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v2/host-databases/:id/admin/create-database`.
/// **PostgreSQL:** requires `X-Pirate-Db-User` / `X-Pirate-Db-Password` (same as host browse; no `PIRATE_POSTGRES_ADMIN_URL`).
/// **MySQL:** still uses host `PIRATE_MYSQL_ADMIN_URL`; db cred headers optional.
pub fn control_api_host_db_v2_admin_create_database_json(
    instance_id: &str,
    database: &str,
    owner: Option<&str>,
    encoding: Option<&str>,
    if_not_exists: bool,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let is_pg = instance_id.split('|').next() == Some("postgresql");
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    if is_pg {
        if matches!(creds, HostDbCreds::None) {
            return Err(
                "database username and password are required for admin create-database (PostgreSQL); set them in the Databases panel"
                    .into(),
            );
        }
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/admin/create-database",
        base,
        enc_path(instance_id)
    );
    let body = serde_json::json!({
        "database": database,
        "owner": owner,
        "encoding": encoding,
        "if_not_exists": if_not_exists,
    });
    let client = crate::http_client::blocking_client()?;
    let mut post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    if let HostDbCreds::Pair(ref u, ref p) = creds {
        post = add_pirate_db_headers(post, u, p);
    }
    let resp = post
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db admin create-database HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v2/host-databases/:id/admin/create-table` — `body_json` is the full request body.
pub fn control_api_host_db_v2_admin_create_table_json(
    instance_id: &str,
    body_json: &str,
) -> Result<String, String> {
    let body_val: serde_json::Value = serde_json::from_str(body_json)
        .map_err(|e| format!("admin create-table: invalid JSON ({e})"))?;
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/admin/create-table",
        base,
        enc_path(instance_id)
    );
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body_val)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db admin create-table HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v2/host-databases/:id/admin/create-user` — `body_json` is the full request body; response may include a generated password once.
/// Per-request DB credentials (same as host-db browse) are required; the server no longer uses `PIRATE_POSTGRES_ADMIN_URL` for this operation.
pub fn control_api_host_db_v2_admin_create_user_json(
    instance_id: &str,
    body_json: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let body_val: serde_json::Value = serde_json::from_str(body_json)
        .map_err(|e| format!("admin create-user: invalid JSON ({e})"))?;
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let (u, p) = match creds {
        HostDbCreds::Pair(ref u, ref p) => (u.as_str(), p.as_str()),
        HostDbCreds::None => {
            return Err(
                "database username and password are required for admin create-user (set in the Databases panel, or use Remember to save the password locally)"
                    .into(),
            );
        }
    };
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/admin/create-user",
        base,
        enc_path(instance_id)
    );
    let client = crate::http_client::blocking_client()?;
    let post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    let post = add_pirate_db_headers(post, u, p);
    let resp = post
        .json(&body_val)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db admin create-user HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v2/host-databases/:id/admin/delete-user` — drop a PostgreSQL login role.
pub fn control_api_host_db_v2_admin_delete_user_json(
    instance_id: &str,
    body_json: &str,
    db_user: Option<&str>,
    db_password: Option<&str>,
) -> Result<String, String> {
    let body_val: serde_json::Value = serde_json::from_str(body_json)
        .map_err(|e| format!("admin delete-user: invalid JSON ({e})"))?;
    let creds = resolve_host_db_creds(instance_id, db_user, db_password)?;
    let (u, p) = match creds {
        HostDbCreds::Pair(ref u, ref p) => (u.as_str(), p.as_str()),
        HostDbCreds::None => {
            return Err(
                "database username and password are required for admin delete-user (set in the Databases panel, or use Remember to save the password locally)"
                    .into(),
            );
        }
    };
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/admin/delete-user",
        base,
        enc_path(instance_id)
    );
    let client = crate::http_client::blocking_client()?;
    let post = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token));
    let post = add_pirate_db_headers(post, u, p);
    let resp = post
        .json(&body_val)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db admin delete-user HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v2/host-databases/:id/migration-run` (whitelisted CLI; JWT only).
pub fn control_api_host_db_v2_migration_run_json(
    instance_id: &str,
    tool: &str,
    workdir: &str,
) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = auth::bearer()?;
    let url = format!(
        "{}/api/v2/host-databases/{}/migration-run",
        base,
        enc_path(instance_id)
    );
    let body = serde_json::json!({ "tool": tool, "workdir": workdir });
    let client = crate::http_client::blocking_client()?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        let _ = clear_control_api_jwt();
        return Err("control-api returned 401; sign in again".into());
    }
    if !status.is_success() {
        let t = resp.text().unwrap_or_default();
        return Err(format!(
            "host-db migration-run HTTP {}: {}",
            status,
            t.chars().take(800).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

#[cfg(test)]
mod format_tests {
    use super::format_bytes_with_mib;

    #[test]
    fn format_bytes_with_mib_includes_mib() {
        let s = format_bytes_with_mib(1_048_576);
        assert!(s.contains("1048576"));
        assert!(s.contains("MiB"));
    }
}
