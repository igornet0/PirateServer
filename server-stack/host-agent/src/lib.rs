//! HTTP API for out-of-band server-stack apply and host reboot.

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use deploy_core::validate_version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;
use subtle::ConstantTimeEq;
use tokio::fs;
use tracing::{info, warn};

#[derive(Clone)]
pub struct AgentConfig {
    pub token: Vec<u8>,
    pub deploy_root: PathBuf,
    pub max_stack_bytes: u64,
}

#[derive(Clone)]
pub struct AgentState {
    pub cfg: AgentConfig,
    pub start: Instant,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub agent_version: &'static str,
    pub server_stack_version: Option<String>,
    pub uptime_sec: u64,
}

#[derive(Deserialize)]
pub struct RebootBody {
    #[serde(default)]
    pub delay_sec: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct RebootAccepted {
    pub status: &'static str,
    pub delay_sec: u64,
}

#[derive(Serialize)]
pub struct StackOkResponse {
    pub status: &'static str,
    pub applied_version: String,
}

#[derive(Serialize)]
pub struct ErrBody {
    pub error: String,
}

fn json_err(status: StatusCode, msg: impl Into<String>) -> Response {
    let body = serde_json::to_string(&ErrBody {
        error: msg.into(),
    })
    .unwrap_or_else(|_| r#"{"error":"serialize"}"#.to_string());
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn bearer_ok(headers: &HeaderMap, token: &[u8]) -> bool {
    let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let prefix = "Bearer ";
    if !auth.starts_with(prefix) {
        return false;
    }
    let t = auth[prefix.len()..].as_bytes();
    if t.len() != token.len() {
        return false;
    }
    t.ct_eq(token).into()
}

fn require_auth(headers: &HeaderMap, token: &[u8]) -> Result<(), Response> {
    if bearer_ok(headers, token) {
        Ok(())
    } else {
        Err(json_err(
            StatusCode::UNAUTHORIZED,
            "missing or invalid Authorization Bearer",
        ))
    }
}

pub fn app(state: AgentState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/reboot", post(reboot))
        .route("/v1/server-stack", post(upload_stack))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "pirate-host-agent",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn status(
    State(state): State<AgentState>,
    headers: HeaderMap,
) -> Result<Json<StatusResponse>, Response> {
    require_auth(&headers, &state.cfg.token)?;
    let ver_path = PathBuf::from(deploy_core::PIRATE_VAR_LIB).join("server-stack-version");
    let server_stack_version = match fs::read_to_string(&ver_path).await {
        Ok(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Err(_) => None,
    };
    Ok(Json(StatusResponse {
        agent_version: env!("CARGO_PKG_VERSION"),
        server_stack_version,
        uptime_sec: state.start.elapsed().as_secs(),
    }))
}

async fn reboot(
    State(state): State<AgentState>,
    headers: HeaderMap,
    Json(body): Json<RebootBody>,
) -> Result<(StatusCode, Json<RebootAccepted>), Response> {
    require_auth(&headers, &state.cfg.token)?;
    if body.delay_sec > 3600 {
        return Err(json_err(
            StatusCode::BAD_REQUEST,
            "delay_sec must be <= 3600",
        ));
    }
    let reason = body.reason.unwrap_or_default();
    audit_line(
        &headers,
        &format!("reboot delay_sec={} reason={}", body.delay_sec, reason),
    );
    schedule_reboot(body.delay_sec)
        .await
        .map_err(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(RebootAccepted {
            status: "scheduled",
            delay_sec: body.delay_sec,
        }),
    ))
}

fn audit_line(headers: &HeaderMap, msg: &str) {
    let peer = headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    info!(target: "pirate_host_agent_audit", peer = %peer, "{}", msg);
}

async fn upload_stack(
    State(state): State<AgentState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<StackOkResponse>, Response> {
    require_auth(&headers, &state.cfg.token)?;
    let ver = headers
        .get("x-pirate-version")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| json_err(StatusCode::BAD_REQUEST, "missing X-Pirate-Version"))?
        .to_string();
    if let Err(e) = validate_version(&ver) {
        return Err(json_err(StatusCode::BAD_REQUEST, e));
    }
    let sha_hex = headers
        .get("x-pirate-sha256")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| json_err(StatusCode::BAD_REQUEST, "missing X-Pirate-Sha256"))?;
    let expected_bytes = hex::decode(sha_hex.trim()).map_err(|_| {
        json_err(
            StatusCode::BAD_REQUEST,
            "invalid X-Pirate-Sha256 (not hex)",
        )
    })?;
    if expected_bytes.len() != 32 {
        return Err(json_err(
            StatusCode::BAD_REQUEST,
            "sha256 must be 32 bytes",
        ));
    }

    audit_line(&headers, &format!("server-stack upload version={}", ver));

    let bytes = to_bytes(
        body,
        state.cfg.max_stack_bytes.saturating_add(1) as usize,
    )
    .await
    .map_err(|e| json_err(StatusCode::BAD_REQUEST, format!("body: {e}")))?;

    if bytes.len() as u64 > state.cfg.max_stack_bytes {
        return Err(json_err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "body exceeds limit of {} bytes",
                state.cfg.max_stack_bytes
            ),
        ));
    }

    let digest = Sha256::digest(&bytes);
    let exp: [u8; 32] = expected_bytes
        .as_slice()
        .try_into()
        .map_err(|_| json_err(StatusCode::BAD_REQUEST, "invalid sha length"))?;
    let da: [u8; 32] = digest.into();
    if !bool::from(da.ct_eq(&exp)) {
        return Err(json_err(StatusCode::BAD_REQUEST, "SHA-256 mismatch"));
    }

    let staging = state.cfg.deploy_root.join(".host-agent-staging");
    fs::create_dir_all(&staging)
        .await
        .map_err(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("staging: {e}")))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_path = staging.join(format!("stack_upload_{stamp}.tar.gz"));
    fs::write(&temp_path, &bytes)
        .await
        .map_err(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("write temp: {e}")))?;

    let extract_dir = staging.join(format!("extract_{}", ver.replace(['/', '\\'], "_")));
    if extract_dir.exists() {
        let _ = fs::remove_dir_all(&extract_dir).await;
    }
    fs::create_dir_all(&extract_dir)
        .await
        .map_err(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, format!("extract dir: {e}")))?;

    let tp = temp_path.clone();
    let ed = extract_dir.clone();
    tokio::task::spawn_blocking(move || unpack_tar_gz(&tp, &ed))
        .await
        .map_err(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| json_err(StatusCode::BAD_REQUEST, format!("unpack: {e}")))?;

    let bundle_root = find_pirate_bundle_root(&extract_dir).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        let _ = std::fs::remove_dir_all(&extract_dir);
        json_err(StatusCode::BAD_REQUEST, e)
    })?;

    let br = bundle_root.clone();
    let ver_clone = ver.clone();
    let status = apply_stack_bundle_command(&br, &ver_clone, None)
        .await
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            let _ = std::fs::remove_dir_all(&extract_dir);
            json_err(StatusCode::INTERNAL_SERVER_ERROR, e)
        })?;

    let _ = fs::remove_file(&temp_path).await;
    let _ = fs::remove_dir_all(&extract_dir).await;

    if !status.success() {
        return Err(json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "apply exited with {}",
                status.code().unwrap_or(-1)
            ),
        ));
    }

    info!(version = %ver, "host-agent server stack applied");
    Ok(Json(StackOkResponse {
        status: "ok",
        applied_version: ver,
    }))
}

fn bundle_has_server_bins(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        dir.join("bin/deploy-server.exe").is_file() && dir.join("bin/control-api.exe").is_file()
    }
    #[cfg(not(windows))]
    {
        dir.join("bin/deploy-server").is_file() && dir.join("bin/control-api").is_file()
    }
}

fn find_pirate_bundle_root(extracted: &Path) -> Result<PathBuf, String> {
    for name in [
        "pirate-linux-amd64",
        "pirate-linux-aarch64",
        "pirate-macos-amd64",
        "pirate-macos-arm64",
        "pirate-windows-amd64",
        "pirate-windows-arm64",
    ] {
        let d = extracted.join(name);
        if bundle_has_server_bins(&d) {
            return Ok(d);
        }
    }
    if bundle_has_server_bins(extracted) {
        return Ok(extracted.to_path_buf());
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(extracted) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() && bundle_has_server_bins(&p) {
                candidates.push(p);
            }
        }
    }
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }
    if candidates.len() > 1 {
        return Err("multiple bundle roots in tarball".into());
    }
    Err("no bundle root with bin/deploy-server and bin/control-api".into())
}

fn unpack_tar_gz(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::process::Command;
        std::fs::create_dir_all(dst)?;
        match Command::new("tar")
            .arg("-xzf")
            .arg(src)
            .arg("-C")
            .arg(dst)
            .status()
        {
            Ok(st) if st.success() => return Ok(()),
            Ok(st) => {
                warn!(?st, path = %src.display(), "system tar failed; falling back to rust tar");
            }
            Err(e) => {
                warn!(%e, path = %src.display(), "system tar not runnable; falling back to rust tar");
            }
        }
    }
    unpack_tar_gz_rust(src, dst)
}

fn unpack_tar_gz_rust(src: &Path, dst: &Path) -> std::io::Result<()> {
    use flate2::read::GzDecoder;
    use std::fs::File;
    use std::path::Component;
    use tar::Archive;

    let file = File::open(src)?;
    let dec = GzDecoder::new(file);
    let mut archive = Archive::new(dec);
    std::fs::create_dir_all(dst)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "path traversal in tar entry",
            ));
        }
        entry.unpack_in(dst)?;
    }
    Ok(())
}

async fn apply_stack_bundle_command(
    br: &Path,
    ver: &str,
    apply_json: Option<&PathBuf>,
) -> Result<std::process::ExitStatus, String> {
    #[cfg(unix)]
    {
        let mut cmd = tokio::process::Command::new("sudo");
        cmd.arg("/usr/local/lib/pirate/pirate-apply-stack-bundle.sh")
            .arg(br)
            .arg(ver);
        if let Some(jp) = apply_json {
            cmd.arg(jp);
        }
        cmd.status()
            .await
            .map_err(|e| format!("sudo apply stack: {e}"))
    }
    #[cfg(windows)]
    {
        let apply_script = std::env::var_os("PIRATE_APPLY_STACK_SCRIPT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let mut p = std::env::var_os("ProgramFiles")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
                p.push("Pirate");
                p.push("lib");
                p.push("pirate");
                p.push("pirate-apply-stack-bundle.ps1");
                p
            });
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&apply_script)
            .arg(br)
            .arg(ver);
        if let Some(jp) = apply_json {
            cmd.arg(jp);
        }
        cmd.status()
            .await
            .map_err(|e| format!("powershell apply stack: {e}"))
    }
}

async fn schedule_reboot(delay_sec: u64) -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut cmd = tokio::process::Command::new("sudo");
        cmd.arg("/usr/local/lib/pirate/pirate-host-agent-reboot.sh")
            .arg(delay_sec.to_string());
        cmd.status()
            .await
            .map_err(|e| format!("sudo reboot script: {e}"))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let delay = delay_sec.min(31536000);
        let status = tokio::process::Command::new("shutdown.exe")
            .arg("/r")
            .arg("/t")
            .arg(delay.to_string())
            .arg("/c")
            .arg("pirate-host-agent reboot")
            .status()
            .await
            .map_err(|e| format!("shutdown: {e}"))?;
        if !status.success() {
            return Err(format!("shutdown exited {}", status.code().unwrap_or(-1)));
        }
        Ok(())
    }
}

/// Run HTTP or HTTPS server depending on `tls` (cert + key paths).
pub async fn run_server(
    addr: SocketAddr,
    state: AgentState,
    tls: Option<(PathBuf, PathBuf)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = app(state);
    if let Some((cert, key)) = tls {
        let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
        info!("listening with TLS on {}", addr);
        axum_server::bind_rustls(addr, config)
            .serve(app.into_make_service())
            .await?;
    } else {
        warn!("TLS not configured: listening plain HTTP (use reverse proxy or PIRATE_HOST_AGENT_TLS_* for production)");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!("listening on {}", addr);
        axum::serve(listener, app).await?;
    }
    Ok(())
}
