//! control-api HTTP: JWT login and server projects overview.

use deploy_core::pirate_project::PirateManifest;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::connection::{
    clear_control_api_jwt, load_control_api_base, load_control_api_jwt, save_control_api_jwt,
};
use futures_util::future::join_all;

fn now_ms() -> i64 {
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
struct LoginResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
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
    pub status_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectsOverview {
    pub projects: Vec<ServerProjectRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn normalize_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

fn fmt_reqwest_send_err(e: reqwest::Error, url: &str) -> String {
    let mut s = format!("{e}");
    if let Some(src) = std::error::Error::source(&e) {
        s.push_str(": ");
        s.push_str(&src.to_string());
    }
    if e.is_connect() {
        s.push_str(
            " — cannot reach control-api at this URL (wrong port/host, firewall, or use the HTTP base nginx exposes; set DEPLOY_CONTROL_API_PUBLIC_URL on deploy-server so GetStatus advertises the right base).",
        );
    } else if e.is_timeout() {
        s.push_str(" — request timed out.");
    }
    if !s.contains(url) {
        s.push_str(&format!(" (url: {url})"));
    }
    s
}

/// POST `/api/v1/auth/login`, store JWT + expiry (`expires_in` seconds).
pub fn control_api_login(base_url: &str, username: &str, password: &str) -> Result<(), String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    let u = username.trim();
    let p = password.trim();
    if u.is_empty() || p.is_empty() {
        return Err("username and password required".into());
    }

    let url = format!("{}/api/v1/auth/login", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "username": u, "password": p }))
        .send()
        .map_err(|e| fmt_reqwest_send_err(e, &url))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "login HTTP {}: {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }

    let login: LoginResponse = resp.json().map_err(|e| e.to_string())?;
    let token = login.access_token.trim().to_string();
    if token.is_empty() {
        return Err("empty access_token in login response".into());
    }

    let expires_at_ms = if login.expires_in > 0 {
        now_ms().saturating_add((login.expires_in as i64).saturating_mul(1000))
    } else {
        // Default 24h if server omits expires_in
        now_ms().saturating_add(86400 * 1000)
    };

    save_control_api_jwt(&token, expires_at_ms)?;
    Ok(())
}

pub fn control_api_logout() -> Result<(), String> {
    clear_control_api_jwt()
}

/// True if a non-empty JWT is stored and not within the pre-expiry grace window (same rules as [`bearer`]).
pub fn control_api_session_active() -> bool {
    let Some((tok, exp)) = load_control_api_jwt() else {
        return false;
    };
    if tok.trim().is_empty() {
        return false;
    }
    if exp > 0 && now_ms() >= exp - 30_000 {
        return false;
    }
    true
}

fn bearer() -> Result<String, String> {
    let Some((tok, exp)) = load_control_api_jwt() else {
        return Err("not logged in to control-api".into());
    };
    if tok.is_empty() {
        return Err("not logged in to control-api".into());
    }
    if exp > 0 && now_ms() >= exp - 30_000 {
        return Err("control-api session expired; sign in again".into());
    }
    Ok(tok)
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
    let base = load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(45))
            .build()
            .map_err(|e| e.to_string())?;

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
                            status_error: None,
                        },
                        Err(e) => ServerProjectRow {
                            id: pid,
                            deploy_root: dr,
                            state: "—".into(),
                            current_version: "—".into(),
                            source: "—".into(),
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
    let base = load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let url = format!("{}/api/v1/projects/allocate", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
pub fn allocate_and_apply_remote_project_id(project_dir: std::path::PathBuf) -> Result<String, String> {
    let id = allocate_remote_project_id()?;
    write_pirate_toml_deploy_project_id(&project_dir, &id)?;
    crate::connection::set_active_project(id.clone())?;
    Ok(id)
}

/// Before deploy/pipeline/rollback: resolve gRPC project id — reuse explicit non-`default`
/// `[project].deploy_project_id` in `pirate.toml`, or keep `default` if that slot already has a real app
/// release (`GetStatus` on `default`: non-empty `current_version` not starting with `stack@`), otherwise
/// allocate a new slot via control-api (requires JWT).
pub fn ensure_deploy_project_id_for_deploy(project_dir: std::path::PathBuf) -> Result<String, String> {
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
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/status", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/projects/telemetry", base);
    let logs_limit_s = logs_limit.max(1).to_string();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .query(&[("project", pid.as_str()), ("logs_limit", logs_limit_s.as_str())])
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
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/projects/telemetry/clear", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/app-env", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/app-env", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/host-deploy-env", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/host-deploy-env", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/host-deploy-env/template", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/nginx/status", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/host-services", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
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

/// `POST /api/v1/host-services/{id}/install`
pub fn control_api_host_service_install(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("service id is empty".into());
    }
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let url = format!("{}/api/v1/host-services/{}/install", base, id);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/host-services/{}/remove", base, id);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
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

/// `GET /api/v1/nginx/site` — JSON (`NginxConfigView`).
pub fn control_api_fetch_nginx_site_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let url = format!("{}/api/v1/nginx/site", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/nginx/site", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
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

/// `POST /api/v1/nginx/ensure` with mode (`api_only` | `with_ui`).
pub fn control_api_ensure_nginx(mode: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let url = format!("{}/api/v1/nginx/ensure", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "mode": mode }))
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
            "nginx ensure HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/process/restart?project=…` — JSON string (`ProcessControlView`).
pub fn control_api_restart_process_json(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/process/restart", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
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
            "restart HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `POST /api/v1/process/stop?project=…` — JSON string (`ProcessControlView`).
pub fn control_api_stop_process_json(project_id: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let pid = project_param(project_id)?;
    let url = format!("{}/api/v1/process/stop", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
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
            "stop HTTP {}: {}",
            status,
            body.chars().take(400).collect::<String>()
        ));
    }
    resp.text().map_err(|e| e.to_string())
}

/// `GET /api/v1/antiddos` — JSON (`AntiddosGetResponse`).
pub fn control_api_antiddos_get_json() -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let url = format!("{}/api/v1/antiddos", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/antiddos", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let url = format!("{}/api/v1/antiddos/stats", base);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
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
pub fn control_api_antiddos_project_put_json(project_id: &str, content: &str) -> Result<String, String> {
    let base =
        load_control_api_base().ok_or_else(|| "control-api base URL is not set".to_string())?;
    let base = normalize_base(&base);
    let token = bearer()?;
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }
    let url = format!(
        "{}/api/v1/antiddos/projects/{}",
        base,
        urlencoding::encode(pid)
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
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
    let token = bearer()?;
    let pid = project_id.trim();
    if pid.is_empty() {
        return Err("project_id is empty".into());
    }
    let url = format!(
        "{}/api/v1/antiddos/projects/{}",
        base,
        urlencoding::encode(pid)
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
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
