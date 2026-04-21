//! HTTP control plane: status (via gRPC to deploy-server), releases (FS), history (PostgreSQL), nginx config.

mod antiddos_api;
mod auth;
mod cors;
mod data_sources_api;
mod error;
mod proxy_sessions_api;
mod proxy_tunnel_redis;
mod host_terminal;

use axum::body::{Body, Bytes};
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use futures::Stream;
use std::collections::HashMap;
use std::convert::Infallible;
use clap::{Parser, Subcommand};
use deploy_auth::{
    load_authorized_peers, load_identity, save_authorized_peers, IdentityFile,
};
use deploy_control::{
    apply_nginx_put, apply_nginx_site_via_sudo, collect_host_services, collect_nginx_status,
    ensure_nginx_via_sudo, host_service_action_via_sudo, read_nginx_config, read_nginx_site_file,
    AllocateProjectResponse, AppEnvView, ControlPlane, CpuDetail, DatabaseColumnsView,
    DatabaseInfoView, DatabaseRelationshipsView, DatabaseSchemasView, DatabaseTablePreviewView,
    DatabaseTablesView, DiskDetail, HostDeployEnvPutView, HostDeployEnvView, HostServiceActionView,
    HostServicesView, HostStatsHistory, MemoryDetail, NginxConfigPut, NginxConfigView, NginxEnsureView,
    NginxEnvUpdateView, NginxEnvVarUpdateView, NginxPutResponseView, NginxStatusView, NetworkDetail,
    ProcessControlView, ProcessesDetail, ProjectsView, RollbackBody, RollbackView, SeriesResponse,
};
use deploy_core::{validate_project_id, validate_version};
use deploy_db::{DbStore, PgPool};
use sqlx::postgres::PgPoolOptions;
use ed25519_dalek::SigningKey;
use error::ApiError;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub(crate) struct ApiState {
    plane: Arc<ControlPlane>,
    nginx_config_path: Option<PathBuf>,
    nginx_test_full_config: bool,
    nginx_admin_token: Option<String>,
    /// When set, `Authorization: Bearer` may match this static token (automation / legacy).
    api_bearer_token: Option<String>,
    /// When set, `Authorization: Bearer` may be a valid HS256 JWT issued by `/api/v1/auth/login`.
    jwt_secret: Option<String>,
    jwt_ttl_secs: u64,
    /// Previous network counters for throughput (overview + detail).
    host_net: Arc<Mutex<Option<deploy_control::NetCounters>>>,
    /// Optional path to tail in `HostStatsView.log_tail` (application log only).
    log_tail_path: Option<PathBuf>,
    /// When true, record samples for `GET /api/v1/host-stats/series`.
    host_stats_series_enabled: bool,
    host_history: Option<Arc<Mutex<HostStatsHistory>>>,
    /// When true, expose SSE and WebSocket host-stats streams (use with care on public networks).
    host_stats_stream_enabled: bool,
    /// `CONTROL_API_HOST_TERMINAL=1`: WebSocket shell at `/api/v1/host-terminal/ws` (Unix only).
    pub(crate) host_terminal_enabled: bool,
    pub(crate) host_terminal_shell: PathBuf,
    pub(crate) host_terminal_session_secs: u64,
    /// Explorer PostgreSQL URL with password removed (for `GET /api/v1/database-info`).
    explorer_connection_display: Option<String>,
    /// Root for DB credential files and optional server-side SMB dirs (`install.sh` creates the tree).
    data_mounts_root: PathBuf,
    smb_mount_script: PathBuf,
    smb_umount_script: PathBuf,
    /// Same Redis as deploy-server (`DEPLOY_REDIS_URL`) for per-session tunnel metrics in inbounds list.
    tunnel_redis: Option<redis::Client>,
    /// Base URL for subscription links (e.g. `https://dash.example.com`). Env: `DEPLOY_SUBSCRIPTION_PUBLIC_HOST` / `CONTROL_API_SUBSCRIPTION_PUBLIC_HOST`.
    subscription_public_base: Option<String>,
    /// Hostname extracted from `subscription_public_base` for Xray `outbound` address.
    subscription_server_hostname: Option<String>,
    /// Optional TLS SNI for Xray export (`DEPLOY_SUBSCRIPTION_TLS_SNI`).
    subscription_tls_sni: Option<String>,
    /// Same as deploy-server `DEPLOY_GRPC_PUBLIC_URL` — public gRPC URL for clients (bootstrap JSON, hints).
    pub(crate) grpc_public_url: Option<String>,
    /// Path to host env file (`EnvironmentFile` for systemd units), default `/etc/pirate-deploy.env`.
    pub(crate) host_deploy_env_path: PathBuf,
    /// Helper run via `sudo -n` to write host env and schedule restarts (`install.sh` deploys it).
    pub(crate) host_deploy_env_write_script: PathBuf,
    /// Effective control-api listen port for env sync fallback.
    control_api_port: u16,
    /// Path to editable nginx site file (`/etc/nginx/sites-available/pirate` by default).
    nginx_site_path: PathBuf,
    /// Helper run via `sudo -n` to install/start nginx and seed Pirate vhost.
    nginx_ensure_script: PathBuf,
    /// Helper run via `sudo -n` to write site config, test and reload nginx.
    nginx_apply_site_script: PathBuf,
    /// Whitelist dispatcher for optional host packages (`install` / `remove`).
    host_service_dispatch_script: PathBuf,
    antiddos_state_dir: PathBuf,
    antiddos_apply_script: PathBuf,
    antiddos_limit_log_path: PathBuf,
    /// `DEPLOY_MAX_UPLOAD_BYTES` for this process; combined with deploy-server `GetStatus.max_upload_bytes` via [`effective_upload_limit_from_config_and_grpc`].
    max_artifact_upload_bytes_configured: u64,
    /// Chunk size when forwarding stored multipart to gRPC.
    deploy_http_chunk_bytes: usize,
    deploy_upload_session_chunk_bytes: usize,
    deploy_upload_session_ttl_secs: u64,
    deploy_upload_sessions: Arc<AsyncMutex<HashMap<String, DeployUploadSessionState>>>,
}

/// Parse database URL and strip password for safe display (PostgreSQL; SQLite returned as-is).
fn redact_database_url(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.starts_with("sqlite:") {
        return Some(t.to_string());
    }
    let mut u = url::Url::parse(t).ok()?;
    let _ = u.set_password(None);
    Some(u.to_string())
}

fn parse_env_lines(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        out.push((key.to_string(), v.trim().to_string()));
    }
    out
}

fn read_env_value(content: &str, key: &str) -> Option<String> {
    for (k, v) in parse_env_lines(content) {
        if k == key {
            return Some(v);
        }
    }
    None
}

fn canonical_http_base(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let mut u = url::Url::parse(t).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    u.set_path("");
    u.set_query(None);
    u.set_fragment(None);
    if u.path() != "/" {
        return None;
    }
    Some(u.to_string().trim_end_matches('/').to_string())
}

fn base_from_grpc_public(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let u = url::Url::parse(t).ok()?;
    let host = u.host_str()?;
    let scheme = if u.scheme() == "https" { "https" } else { "http" };
    Some(format!("{scheme}://{host}"))
}

fn apply_nginx_env_sync(
    current_content: &str,
    mode: &str,
    fallback_control_port: u16,
) -> Result<(String, Vec<NginxEnvVarUpdateView>), ApiError> {
    let mut desired: Vec<(&str, Option<String>)> = Vec::new();
    let control_port = read_env_value(current_content, "CONTROL_API_PORT")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(fallback_control_port);
    let direct_url = format!("http://127.0.0.1:{control_port}");
    match mode {
        "api_only" | "with_ui" => {
            let public_from_current = read_env_value(current_content, "DEPLOY_CONTROL_API_PUBLIC_URL")
                .and_then(|s| canonical_http_base(&s));
            let public_from_grpc = read_env_value(current_content, "DEPLOY_GRPC_PUBLIC_URL")
                .and_then(|s| base_from_grpc_public(&s));
            let public_url = public_from_current.or(public_from_grpc).unwrap_or_else(|| direct_url.clone());
            desired.push(("NGINX_CONFIG_PATH", Some("/etc/nginx/nginx.conf".to_string())));
            desired.push(("DEPLOY_CONTROL_API_PUBLIC_URL", Some(public_url)));
            desired.push(("DEPLOY_CONTROL_API_DIRECT_URL", Some(direct_url)));
        }
        "remove" => {
            desired.push(("NGINX_CONFIG_PATH", None));
            desired.push(("NGINX_TEST_FULL_CONFIG", None));
            desired.push(("NGINX_ADMIN_TOKEN", None));
            desired.push(("DEPLOY_CONTROL_API_PUBLIC_URL", Some(direct_url.clone())));
            desired.push(("DEPLOY_CONTROL_API_DIRECT_URL", Some(direct_url)));
        }
        _ => {
            return Err(ApiError::bad_request(
                "mode must be api_only, with_ui or remove",
            ));
        }
    }

    let mut updates = Vec::<NginxEnvVarUpdateView>::new();
    let mut out_lines = Vec::<String>::new();
    let mut seen = std::collections::HashSet::<String>::new();
    let mut desired_map = std::collections::HashMap::<String, Option<String>>::new();
    for (k, v) in desired {
        desired_map.insert(k.to_string(), v);
    }

    for line in current_content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || !trimmed.contains('=') {
            out_lines.push(line.to_string());
            continue;
        }
        let Some((left, right)) = line.split_once('=') else {
            out_lines.push(line.to_string());
            continue;
        };
        let key = left.trim().to_string();
        if let Some(new_val_opt) = desired_map.get(&key) {
            seen.insert(key.clone());
            let old_val = Some(right.trim().to_string());
            match new_val_opt {
                Some(new_val) => {
                    out_lines.push(format!("{key}={new_val}"));
                    if old_val.as_deref() != Some(new_val.as_str()) {
                        updates.push(NginxEnvVarUpdateView {
                            key,
                            old_value: old_val,
                            new_value: Some(new_val.clone()),
                        });
                    }
                }
                None => {
                    updates.push(NginxEnvVarUpdateView {
                        key,
                        old_value: old_val,
                        new_value: None,
                    });
                }
            }
        } else {
            out_lines.push(line.to_string());
        }
    }

    for (key, new_val_opt) in desired_map {
        if seen.contains(&key) {
            continue;
        }
        if let Some(new_val) = new_val_opt {
            out_lines.push(format!("{key}={new_val}"));
            updates.push(NginxEnvVarUpdateView {
                key,
                old_value: None,
                new_value: Some(new_val),
            });
        }
    }

    let mut next = out_lines.join("\n");
    if !next.ends_with('\n') {
        next.push('\n');
    }
    Ok((next, updates))
}

fn bearer_raw(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
}

fn token_matches(state: &ApiState, token: &str) -> bool {
    if let Some(ref static_tok) = state.api_bearer_token {
        if token == static_tok.as_str() {
            return true;
        }
    }
    if let Some(ref secret) = state.jwt_secret {
        if auth::decode_access_token(token, secret).is_ok() {
            return true;
        }
    }
    false
}

pub(crate) fn check_api_bearer(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let require_auth = state.api_bearer_token.is_some() || state.jwt_secret.is_some();
    if !require_auth {
        return Ok(());
    }
    let Some(token) = bearer_raw(headers) else {
        return Err(ApiError::unauthorized(
            "missing or invalid Authorization Bearer token",
        ));
    };
    if token_matches(state, token) {
        Ok(())
    } else {
        Err(ApiError::unauthorized(
            "missing or invalid Authorization Bearer token",
        ))
    }
}

/// Same as [`check_api_bearer`], but also accepts `access_token` query (for EventSource / WebSocket).
pub(crate) fn check_api_bearer_with_query(
    state: &ApiState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<(), ApiError> {
    let require_auth = state.api_bearer_token.is_some() || state.jwt_secret.is_some();
    if !require_auth {
        return Ok(());
    }
    if let Some(t) = bearer_raw(headers) {
        return if token_matches(state, t) {
            Ok(())
        } else {
            Err(ApiError::unauthorized(
                "invalid Authorization Bearer token",
            ))
        };
    }
    if let Some(t) = query_token {
        if token_matches(state, t) {
            return Ok(());
        }
    }
    Err(ApiError::unauthorized(
        "missing or invalid token (Authorization Bearer or access_token query for streams)",
    ))
}

fn check_nginx_write_auth(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    match &state.nginx_admin_token {
        None => Ok(()),
        Some(tok) => {
            let x_ok = headers
                .get("x-nginx-admin-token")
                .and_then(|v| v.to_str().ok())
                == Some(tok.as_str());
            let bearer_ok = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|a| a == format!("Bearer {}", tok));
            if x_ok || bearer_ok {
                Ok(())
            } else {
                Err(ApiError::unauthorized(
                    "missing or invalid nginx admin token (use X-Nginx-Admin-Token or Authorization Bearer)",
                ))
            }
        }
    }
}

async fn health() -> &'static str {
    "ok"
}

/// Max download payload for `GET /api/v1/ping?bytes=` (aligns with deploy ConnectionProbe caps).
const PING_MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(serde::Deserialize)]
struct PingQuery {
    /// If omitted or zero: JSON pong. If positive: octet-stream body of this size (capped).
    #[serde(default)]
    bytes: Option<u64>,
}

/// Unauthenticated reachability + optional bandwidth sample for `pirate ping`.
async fn api_ping(Query(q): Query<PingQuery>) -> impl IntoResponse {
    let raw = q.bytes.unwrap_or(0);
    let n = raw.min(PING_MAX_BYTES);
    if n == 0 {
        let server_ms = chrono::Utc::now().timestamp_millis();
        return Json(serde_json::json!({
            "pong": true,
            "server_ms": server_ms,
        }))
        .into_response();
    }
    let body = vec![0u8; n as usize];
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(body))
        .unwrap()
        .into_response()
}

#[derive(serde::Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(serde::Serialize)]
struct LoginResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

async fn api_login(
    State(s): State<ApiState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<LoginResponse>, ApiError> {
    let Some(ref jwt_sec) = s.jwt_secret else {
        return Err(ApiError::service_unavailable(
            "CONTROL_API_JWT_SECRET is not set",
        ));
    };
    let Some(ref db) = s.plane.db else {
        return Err(ApiError::service_unavailable(
            "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)",
        ));
    };
    let u = body.username.trim();
    if u.is_empty() || body.password.is_empty() {
        return Err(ApiError::bad_request("username and password required"));
    }
    let row = db
        .find_dashboard_user_by_username(u)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some(row) = row else {
        return Err(ApiError::unauthorized("invalid username or password"));
    };
    if !auth::verify_password_against_hash(&body.password, &row.password_hash) {
        return Err(ApiError::unauthorized("invalid username or password"));
    }
    let token = auth::encode_access_token(row.id, jwt_sec, s.jwt_ttl_secs)
        .map_err(ApiError::internal)?;
    Ok(Json(LoginResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        expires_in: s.jwt_ttl_secs,
    }))
}

#[derive(serde::Deserialize)]
struct ProjectQuery {
    /// Empty or omitted means `default`.
    #[serde(default)]
    project: String,
}

#[derive(serde::Deserialize)]
struct ProjectTelemetryQuery {
    /// Empty or omitted means `default`.
    #[serde(default)]
    project: String,
    /// Max number of log lines to return.
    #[serde(default)]
    logs_limit: Option<usize>,
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    /// When omitted or empty, return events for all projects.
    #[serde(default)]
    project: Option<String>,
}

pub(crate) fn project_or_default(project: &str) -> String {
    if project.trim().is_empty() {
        "default".to_string()
    } else {
        project.trim().to_string()
    }
}

/// Effective max artifact size for HTTP deploy paths: `min(control-api env, deploy-server GetStatus)` when the latter is known.
pub(crate) fn effective_upload_limit_from_config_and_grpc(
    configured: u64,
    grpc_max: Option<u64>,
) -> u64 {
    match grpc_max {
        Some(g) if g > 0 => configured.min(g),
        _ => configured,
    }
}

async fn effective_artifact_upload_limit(s: &ApiState, project_id: &str) -> (u64, Option<u64>) {
    let cfg = s.max_artifact_upload_bytes_configured;
    let pid = project_or_default(project_id);
    match s.plane.get_status(&pid).await {
        Ok(st) => {
            let grpc = st.max_upload_bytes;
            let eff = effective_upload_limit_from_config_and_grpc(cfg, grpc);
            if let Some(g) = grpc {
                if g != cfg {
                    warn!(
                        project_id = %pid,
                        control_api_max_upload_bytes = cfg,
                        deploy_server_max_upload_bytes = g,
                        effective_max_upload_bytes = eff,
                        "artifact upload limit: control-api DEPLOY_MAX_UPLOAD_BYTES and deploy-server GetStatus differ; enforcing min()"
                    );
                }
            }
            (eff, grpc)
        }
        Err(e) => {
            warn!(
                %e,
                project_id = %pid,
                "GetStatus failed; artifact upload limit uses control-api DEPLOY_MAX_UPLOAD_BYTES only"
            );
            (cfg, None)
        }
    }
}

async fn api_status(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<deploy_control::StatusView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let pid = project_or_default(&q.project);
    let mut st = s.plane.get_status(&pid).await.map_err(ApiError::from)?;
    let cfg = s.max_artifact_upload_bytes_configured;
    let grpc = st.max_upload_bytes;
    let eff = effective_upload_limit_from_config_and_grpc(cfg, grpc);
    if let Some(g) = grpc {
        if g != cfg {
            warn!(
                project_id = %pid,
                control_api_max_upload_bytes = cfg,
                deploy_server_max_upload_bytes = g,
                effective_max_upload_bytes = eff,
                "artifact upload limit: control-api DEPLOY_MAX_UPLOAD_BYTES and deploy-server GetStatus differ; enforcing min()"
            );
        }
    }
    st.max_upload_bytes = Some(eff);
    Ok(Json(st))
}

/// Multipart deploy: fields `version` (text), optional `manifest_toml` (text, before `artifact`), `artifact` (file, `.tar.gz` bytes).
/// Send `version` and optional `manifest_toml` before `artifact` so the server parses them first.
#[derive(serde::Serialize)]
struct DeployArtifactOut {
    status: String,
    deployed_version: String,
}

#[derive(Clone, Debug)]
struct DeployUploadSessionState {
    project_id: String,
    version: String,
    manifest_toml: Option<String>,
    artifact_path: PathBuf,
    artifact_bytes: u64,
    artifact_sha256: String,
    received_bytes: u64,
    updated_at_ms: i64,
}

#[derive(serde::Deserialize)]
struct DeployUploadSessionCreateBody {
    version: String,
    manifest_toml: Option<String>,
    artifact_bytes: u64,
    artifact_sha256: String,
}

#[derive(serde::Serialize)]
struct DeployUploadSessionCreateOut {
    upload_id: String,
    chunk_bytes: usize,
    received_bytes: u64,
}

#[derive(serde::Serialize)]
struct DeployUploadSessionChunkOut {
    received_bytes: u64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn session_is_expired(sess: &DeployUploadSessionState, ttl_secs: u64, now_ms: i64) -> bool {
    let ttl_ms = (ttl_secs as i64).saturating_mul(1000);
    now_ms.saturating_sub(sess.updated_at_ms) > ttl_ms
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

async fn cleanup_expired_upload_sessions(state: &ApiState) {
    let now = now_ms();
    let mut expired_paths = Vec::<PathBuf>::new();
    {
        let mut sessions = state.deploy_upload_sessions.lock().await;
        sessions.retain(|_, sess| {
            if session_is_expired(sess, state.deploy_upload_session_ttl_secs, now) {
                expired_paths.push(sess.artifact_path.clone());
                false
            } else {
                true
            }
        });
    }
    for p in expired_paths {
        let _ = tokio::fs::remove_file(&p).await;
    }
}

async fn api_deploy_artifact_session_create(
    State(s): State<ApiState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<DeployUploadSessionCreateBody>,
) -> Result<Json<DeployUploadSessionCreateOut>, ApiError> {
    cleanup_expired_upload_sessions(&s).await;
    check_api_bearer(&s, &headers)?;
    validate_project_id(&project_id).map_err(|e| ApiError::bad_request(e.to_string()))?;
    validate_version(&body.version).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let (eff, grpc) = effective_artifact_upload_limit(&s, &project_id).await;
    if body.artifact_bytes == 0 || body.artifact_bytes > eff {
        return Err(ApiError::bad_request_artifact_limit(
            format!("artifact_bytes must be between 1 and {}", eff),
            s.max_artifact_upload_bytes_configured,
            grpc,
            eff,
        ));
    }
    if !is_sha256_hex(body.artifact_sha256.as_str()) {
        return Err(ApiError::bad_request(
            "artifact_sha256 must be a 64-char hex string",
        ));
    }

    let upload_id = uuid::Uuid::new_v4().to_string();
    let artifact_path =
        std::env::temp_dir().join(format!("pirate-session-deploy-{}.tar.gz", upload_id));
    tokio::fs::File::create(&artifact_path)
        .await
        .map_err(|e| ApiError::internal(format!("create upload session temp file: {e}")))?;
    let now = now_ms();
    let session = DeployUploadSessionState {
        project_id: project_id.clone(),
        version: body.version,
        manifest_toml: body.manifest_toml,
        artifact_path,
        artifact_bytes: body.artifact_bytes,
        artifact_sha256: body.artifact_sha256.to_lowercase(),
        received_bytes: 0,
        updated_at_ms: now,
    };
    {
        let mut sessions = s.deploy_upload_sessions.lock().await;
        sessions.insert(upload_id.clone(), session);
    }

    info!(
        upload_id = %upload_id,
        project_id = %project_id,
        artifact_bytes = body.artifact_bytes,
        "deploy_artifact session created"
    );

    Ok(Json(DeployUploadSessionCreateOut {
        upload_id,
        chunk_bytes: s.deploy_upload_session_chunk_bytes,
        received_bytes: 0,
    }))
}

async fn api_deploy_artifact_session_chunk(
    State(s): State<ApiState>,
    Path((project_id, upload_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<DeployUploadSessionChunkOut>, ApiError> {
    cleanup_expired_upload_sessions(&s).await;
    check_api_bearer(&s, &headers)?;
    validate_project_id(&project_id).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let offset: u64 = query
        .get("offset")
        .ok_or_else(|| ApiError::bad_request("missing query parameter `offset`"))?
        .parse()
        .map_err(|_| ApiError::bad_request("invalid query parameter `offset`"))?;

    let snapshot = {
        let mut sessions = s.deploy_upload_sessions.lock().await;
        let Some(sess) = sessions.get(&upload_id).cloned() else {
            return Err(ApiError::bad_request("unknown upload session"));
        };
        if sess.project_id != project_id {
            return Err(ApiError::bad_request("upload session does not belong to project"));
        }
        let now = now_ms();
        if session_is_expired(&sess, s.deploy_upload_session_ttl_secs, now) {
            let removed = sessions.remove(&upload_id);
            drop(sessions);
            if let Some(r) = removed {
                let _ = tokio::fs::remove_file(&r.artifact_path).await;
            }
            return Err(ApiError::bad_request("upload session expired"));
        }
        sess
    };

    if offset != snapshot.received_bytes {
        return Err(ApiError::bad_request(format!(
            "invalid offset: expected current={} got offset={}",
            snapshot.received_bytes, offset
        )));
    }
    if body.is_empty() {
        return Err(ApiError::bad_request("chunk body must not be empty"));
    }
    let next = offset.saturating_add(body.len() as u64);
    if next > snapshot.artifact_bytes {
        return Err(ApiError::bad_request(format!(
            "chunk exceeds declared artifact_bytes (declared={}, attempted_end={})",
            snapshot.artifact_bytes, next
        )));
    }
    if let Some(h) = headers.get("x-chunk-sha256") {
        let expected = h
            .to_str()
            .map_err(|_| ApiError::bad_request("invalid x-chunk-sha256 header"))?;
        if !is_sha256_hex(expected) {
            return Err(ApiError::bad_request(
                "x-chunk-sha256 must be a 64-char hex string",
            ));
        }
        let actual = format!("{:x}", Sha256::digest(&body));
        if actual != expected.to_lowercase() {
            return Err(ApiError::bad_request("x-chunk-sha256 mismatch"));
        }
    }

    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&snapshot.artifact_path)
        .await
        .map_err(|e| ApiError::internal(format!("artifact append open: {e}")))?;
    f.write_all(&body)
        .await
        .map_err(|e| ApiError::internal(format!("artifact append write: {e}")))?;
    f.sync_all()
        .await
        .map_err(|e| ApiError::internal(format!("artifact append fsync: {e}")))?;

    debug!(
        upload_id = %upload_id,
        project_id = %project_id,
        offset,
        chunk_len = body.len(),
        "deploy_artifact session chunk stored"
    );

    let now = now_ms();
    let updated = {
        let mut sessions = s.deploy_upload_sessions.lock().await;
        let Some(sess) = sessions.get_mut(&upload_id) else {
            return Err(ApiError::bad_request("unknown upload session"));
        };
        if sess.received_bytes != offset {
            return Err(ApiError::bad_request(format!(
                "invalid offset: expected current={} got offset={}",
                sess.received_bytes, offset
            )));
        }
        sess.received_bytes = next;
        sess.updated_at_ms = now;
        sess.received_bytes
    };
    Ok(Json(DeployUploadSessionChunkOut {
        received_bytes: updated,
    }))
}

async fn api_deploy_artifact_session_complete(
    State(s): State<ApiState>,
    Path((project_id, upload_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<DeployArtifactOut>, ApiError> {
    cleanup_expired_upload_sessions(&s).await;
    check_api_bearer(&s, &headers)?;
    validate_project_id(&project_id).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let sess = {
        let mut sessions = s.deploy_upload_sessions.lock().await;
        sessions
            .remove(&upload_id)
            .ok_or_else(|| ApiError::bad_request("unknown upload session"))?
    };
    if sess.project_id != project_id {
        let _ = tokio::fs::remove_file(&sess.artifact_path).await;
        return Err(ApiError::bad_request("upload session does not belong to project"));
    }
    if session_is_expired(&sess, s.deploy_upload_session_ttl_secs, now_ms()) {
        let _ = tokio::fs::remove_file(&sess.artifact_path).await;
        return Err(ApiError::bad_request("upload session expired"));
    }
    if sess.received_bytes != sess.artifact_bytes {
        let _ = tokio::fs::remove_file(&sess.artifact_path).await;
        return Err(ApiError::bad_request(format!(
            "upload incomplete: received={} expected={}",
            sess.received_bytes, sess.artifact_bytes
        )));
    }

    let mut f = tokio::fs::File::open(&sess.artifact_path)
        .await
        .map_err(|e| ApiError::internal(format!("artifact open for sha256: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| ApiError::internal(format!("artifact read for sha256: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_sha256 = format!("{:x}", hasher.finalize());
    if actual_sha256 != sess.artifact_sha256 {
        let _ = tokio::fs::remove_file(&sess.artifact_path).await;
        return Err(ApiError::bad_request(
            "artifact sha256 mismatch at complete step",
        ));
    }

    info!(
        upload_id = %upload_id,
        project_id = %project_id,
        artifact_bytes = sess.artifact_bytes,
        "deploy_artifact session complete: forwarding to deploy-server"
    );

    let (eff, _) = effective_artifact_upload_limit(&s, &sess.project_id).await;
    let result = s
        .plane
        .grpc_upload_project_artifact_from_path(
            &sess.project_id,
            &sess.version,
            sess.manifest_toml.as_deref(),
            &sess.artifact_path,
            sess.artifact_bytes,
            s.deploy_http_chunk_bytes,
            eff,
        )
        .await
        .map_err(ApiError::from);
    let _ = tokio::fs::remove_file(&sess.artifact_path).await;
    let r = result?;

    Ok(Json(DeployArtifactOut {
        status: r.status,
        deployed_version: r.deployed_version,
    }))
}

#[derive(serde::Serialize)]
struct OkOut {
    ok: bool,
}

async fn api_deploy_artifact_session_delete(
    State(s): State<ApiState>,
    Path((project_id, upload_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<OkOut>, ApiError> {
    cleanup_expired_upload_sessions(&s).await;
    check_api_bearer(&s, &headers)?;
    validate_project_id(&project_id).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let removed = {
        let mut sessions = s.deploy_upload_sessions.lock().await;
        sessions.remove(&upload_id)
    };
    if let Some(sess) = removed {
        if sess.project_id != project_id {
            return Err(ApiError::bad_request("upload session does not belong to project"));
        }
        let _ = tokio::fs::remove_file(&sess.artifact_path).await;
    }
    Ok(Json(OkOut { ok: true }))
}

async fn api_deploy_artifact_multipart(
    State(s): State<ApiState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<DeployArtifactOut>, ApiError> {
    check_api_bearer(&s, &headers)?;
    validate_project_id(&project_id).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let (eff, grpc) = effective_artifact_upload_limit(&s, &project_id).await;

    let mut version: Option<String> = None;
    let mut manifest: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("multipart: {e}")))?
    {
        match field.name() {
            Some("version") => {
                if version.is_some() {
                    return Err(ApiError::bad_request("duplicate multipart field `version`"));
                }
                let t = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("version field: {e}")))?;
                validate_version(&t).map_err(|e| ApiError::bad_request(e.to_string()))?;
                version = Some(t);
            }
            Some("manifest_toml") => {
                if manifest.is_some() {
                    return Err(ApiError::bad_request(
                        "duplicate multipart field `manifest_toml`",
                    ));
                }
                let t = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("manifest_toml field: {e}")))?;
                if t.len() > 2 * 1024 * 1024 {
                    return Err(ApiError::bad_request(
                        "manifest_toml field too large (max 2 MiB)",
                    ));
                }
                manifest = Some(t);
            }
            Some("artifact") => {
                if version.is_none() {
                    return Err(ApiError::bad_request(
                        "send multipart field `version` before `artifact`",
                    ));
                }
                let ver = version.clone().expect("checked");
                let path = std::env::temp_dir().join(format!(
                    "pirate-multipart-deploy-{}.tar.gz",
                    uuid::Uuid::new_v4()
                ));
                let mut f = tokio::fs::File::create(&path)
                    .await
                    .map_err(|e| ApiError::internal(format!("temp artifact: {e}")))?;
                let mut total: u64 = 0;
                let max = eff;
                loop {
                    let chunk = field.chunk()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("artifact read: {e}")))?;
                    let Some(chunk) = chunk else {
                        break;
                    };
                    total += chunk.len() as u64;
                    if total > max {
                        drop(f);
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(ApiError::bad_request_artifact_limit(
                            format!("artifact exceeds limit of {} bytes", max),
                            s.max_artifact_upload_bytes_configured,
                            grpc,
                            eff,
                        ));
                    }
                    f.write_all(&chunk)
                        .await
                        .map_err(|e| ApiError::internal(format!("artifact write: {e}")))?;
                }
                f.sync_all()
                    .await
                    .map_err(|e| ApiError::internal(format!("artifact fsync: {e}")))?;

                let r = s
                    .plane
                    .grpc_upload_project_artifact_from_path(
                        &project_id,
                        &ver,
                        manifest.as_deref(),
                        &path,
                        total,
                        s.deploy_http_chunk_bytes,
                        max,
                    )
                    .await
                    .map_err(ApiError::from);
                let _ = tokio::fs::remove_file(&path).await;
                let r = r?;
                return Ok(Json(DeployArtifactOut {
                    status: r.status,
                    deployed_version: r.deployed_version,
                }));
            }
            _ => {
                let _ = field.text().await;
            }
        }
    }

    Err(ApiError::bad_request(
        "missing multipart field `artifact` (and preceding `version`)",
    ))
}

async fn api_project_telemetry(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectTelemetryQuery>,
) -> Result<Json<deploy_control::ProjectTelemetryView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .project_telemetry(&project_or_default(&q.project), q.logs_limit.unwrap_or(120))
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(serde::Serialize)]
struct TelemetryClearOut {
    ok: bool,
}

async fn api_project_telemetry_clear(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<TelemetryClearOut>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .clear_project_runtime_log(&project_or_default(&q.project))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(TelemetryClearOut { ok: true }))
}

#[derive(serde::Serialize)]
struct BootstrapHintsOut {
    grpc_public_url: Option<String>,
}

/// Authenticated: exposes `DEPLOY_GRPC_PUBLIC_URL` for UI exports (e.g. Inbounds Copy JSON).
async fn api_bootstrap_hints(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<BootstrapHintsOut>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(BootstrapHintsOut {
        grpc_public_url: s.grpc_public_url.clone(),
    }))
}

async fn api_database_info(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<DatabaseInfoView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .database_info(s.explorer_connection_display.clone())
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn api_database_schemas(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<DatabaseSchemasView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane.database_schemas().await.map(Json).map_err(Into::into)
}

#[derive(serde::Deserialize)]
struct DatabaseTablesQuery {
    schema: String,
}

async fn api_database_tables(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<DatabaseTablesQuery>,
) -> Result<Json<DatabaseTablesView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    if q.schema.trim().is_empty() {
        return Err(ApiError::bad_request("query parameter `schema` is required"));
    }
    s.plane
        .database_tables(q.schema.trim())
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn api_database_columns(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path((schema, table)): Path<(String, String)>,
) -> Result<Json<DatabaseColumnsView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .database_columns(&schema, &table)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(serde::Deserialize)]
struct TablePreviewQuery {
    #[serde(default = "default_preview_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_preview_limit() -> i64 {
    100
}

async fn api_database_table_rows(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path((schema, table)): Path<(String, String)>,
    Query(q): Query<TablePreviewQuery>,
) -> Result<Json<DatabaseTablePreviewView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .database_table_preview(&schema, &table, q.limit, q.offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn api_database_relationships(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<DatabaseRelationshipsView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .database_relationships()
        .await
        .map(Json)
        .map_err(Into::into)
}

fn host_stats_snapshot_blocking(state: &ApiState) -> deploy_control::HostStatsView {
    let root = state.plane.deploy_root().to_path_buf();
    let log_path = state.log_tail_path.clone();
    let prev = state.host_net.lock().unwrap().clone();
    let (stats, net) =
        deploy_control::collect_host_stats(&root, prev.as_ref(), log_path.as_deref());
    {
        let mut g = state.host_net.lock().unwrap();
        *g = Some(net);
    }
    if state.host_stats_series_enabled {
        if let Some(h) = &state.host_history {
            let ts_ms = chrono::Utc::now().timestamp_millis();
            let net_rx: f64 = stats
                .network_interfaces
                .iter()
                .map(|i| i.rx_bytes_per_s)
                .sum();
            let net_tx: f64 = stats
                .network_interfaces
                .iter()
                .map(|i| i.tx_bytes_per_s)
                .sum();
            let mut hist = h.lock().unwrap();
            hist.record(
                ts_ms,
                stats.cpu_usage_percent,
                stats.memory_used_bytes,
                stats.load_average_1m,
                net_rx,
                net_tx,
            );
        }
    }
    stats
}

async fn api_host_stats(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<deploy_control::HostStatsView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let state = s.clone();
    let stats = tokio::task::spawn_blocking(move || host_stats_snapshot_blocking(&state))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(stats))
}

#[derive(serde::Deserialize)]
struct HostStatsDetailQuery {
    #[serde(default = "default_top_n")]
    top: usize,
}

fn default_top_n() -> usize {
    20
}

#[derive(serde::Deserialize)]
struct ProcessesDetailQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_proc_limit")]
    limit: usize,
}

fn default_proc_limit() -> usize {
    200
}

#[derive(serde::Deserialize)]
struct SeriesQuery {
    metric: String,
    #[serde(default = "default_series_range")]
    range: String,
}

fn default_series_range() -> String {
    "1h".to_string()
}

async fn api_host_stats_detail_cpu(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<HostStatsDetailQuery>,
) -> Result<Json<CpuDetail>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let top = q.top.clamp(5, 100);
    let detail = tokio::task::spawn_blocking(move || deploy_control::collect_cpu_detail(top))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(detail))
}

async fn api_host_stats_detail_memory(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<HostStatsDetailQuery>,
) -> Result<Json<MemoryDetail>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let top = q.top.clamp(5, 100);
    let detail = tokio::task::spawn_blocking(move || deploy_control::collect_memory_detail(top))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(detail))
}

async fn api_host_stats_detail_disk(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<HostStatsDetailQuery>,
) -> Result<Json<DiskDetail>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let top = q.top.clamp(5, 100);
    let detail = tokio::task::spawn_blocking(move || deploy_control::collect_disk_detail(top))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(detail))
}

async fn api_host_stats_detail_network(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<NetworkDetail>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let prev = s.host_net.lock().unwrap().clone();
    let (detail, net) = tokio::task::spawn_blocking(move || {
        deploy_control::collect_network_detail(prev.as_ref())
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    {
        let mut g = s.host_net.lock().unwrap();
        *g = Some(net);
    }
    Ok(Json(detail))
}

async fn api_host_stats_detail_processes(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProcessesDetailQuery>,
) -> Result<Json<ProcessesDetail>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let limit = q.limit.clamp(10, 2000);
    let qstr = q.q.clone();
    let detail = tokio::task::spawn_blocking(move || {
        deploy_control::collect_processes_list(&qstr, limit)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(detail))
}

async fn api_host_stats_series(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<SeriesQuery>,
) -> Result<Json<SeriesResponse>, ApiError> {
    check_api_bearer(&s, &headers)?;
    if !s.host_stats_series_enabled {
        return Err(ApiError::service_unavailable(
            "CONTROL_API_HOST_STATS_SERIES is not enabled",
        ));
    }
    let Some(h) = &s.host_history else {
        return Err(ApiError::service_unavailable("series buffer not configured"));
    };
    let hist = h.lock().unwrap();
    let res = hist.series(&q.metric, &q.range);
    Ok(Json(res))
}

#[derive(serde::Deserialize)]
pub(crate) struct StreamAuthQuery {
    access_token: Option<String>,
}

async fn api_host_stats_sse(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<StreamAuthQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    check_api_bearer_with_query(&s, &headers, q.access_token.as_deref())?;
    if !s.host_stats_stream_enabled {
        return Err(ApiError::service_unavailable(
            "CONTROL_API_HOST_STATS_STREAM is not enabled",
        ));
    }
    let state = s.clone();
    let stream = async_stream::stream! {
        // First snapshot immediately (EventSource / fetch clients see data without a 5s wait).
        let st0 = state.clone();
        let json0 = tokio::task::spawn_blocking(move || {
            let h = host_stats_snapshot_blocking(&st0);
            serde_json::to_string(&h).ok()
        })
        .await
        .ok()
        .flatten();
        if let Some(payload) = json0 {
            yield Ok(Event::default().data(payload));
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let st = state.clone();
            let json = tokio::task::spawn_blocking(move || {
                let h = host_stats_snapshot_blocking(&st);
                serde_json::to_string(&h).ok()
            })
            .await
            .ok()
            .flatten();
            if let Some(payload) = json {
                yield Ok(Event::default().data(payload));
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15)),
    ))
}

async fn api_host_stats_ws(
    ws: WebSocketUpgrade,
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<StreamAuthQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_api_bearer_with_query(&s, &headers, q.access_token.as_deref())?;
    if !s.host_stats_stream_enabled {
        return Err(ApiError::service_unavailable(
            "CONTROL_API_HOST_STATS_STREAM is not enabled",
        ));
    }
    let state = s.clone();
    Ok(ws.on_upgrade(move |socket| host_stats_ws_task(socket, state)))
}

async fn host_stats_ws_task(mut socket: WebSocket, state: ApiState) {
    use axum::extract::ws::Message;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        let st = state.clone();
        let payload = tokio::task::spawn_blocking(move || {
            serde_json::to_string(&host_stats_snapshot_blocking(&st)).ok()
        })
        .await
        .ok()
        .flatten();
        let Some(text) = payload else {
            continue;
        };
        if socket.send(Message::Text(text)).await.is_err() {
            break;
        }
    }
}

async fn api_releases(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<deploy_control::ReleasesView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .list_releases(&project_or_default(&q.project))
        .map(Json)
        .map_err(Into::into)
}

async fn api_projects(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<ProjectsView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(s.plane.list_projects()))
}

/// Allocate a new deploy project id (filesystem + metadata DB); for local clients to fill `pirate.toml`.
async fn api_projects_allocate(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AllocateProjectResponse>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .allocate_project_id()
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn api_history(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<deploy_control::HistoryView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let filter = q
        .project
        .as_ref()
        .map(|p| p.as_str())
        .filter(|p| !p.is_empty());
    s.plane
        .fetch_history(100, filter)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(serde::Deserialize)]
struct GrpcSessionsQuery {
    #[serde(default)]
    limit: Option<i64>,
    /// Seconds within which a peer counts as "online" (default 120, clamped 10–86400).
    #[serde(default)]
    online_secs: Option<i64>,
    /// When true, `recent` includes `tcp_open` / `tcp_close` rows (verbose audit).
    #[serde(default)]
    include_tcp_audit: bool,
}

#[derive(serde::Serialize)]
struct GrpcSessionsSummaryView {
    total_events: i64,
    tcp_open_total: i64,
    tcp_close_total: i64,
    /// Best-effort: max(0, tcp_open_total − tcp_close_total).
    estimated_open_tcp: i64,
    /// Same as tcp_close_total (each `tcp_close` row is one logged disconnect).
    closed_tcp_events: i64,
    by_kind: HashMap<String, i64>,
}

#[derive(serde::Serialize)]
struct GrpcSessionEventView {
    id: i64,
    created_at: chrono::DateTime<chrono::Utc>,
    kind: String,
    peer_ip: String,
    status: String,
    grpc_method: String,
    client_public_key_b64: Option<String>,
    detail: String,
}

#[derive(serde::Serialize)]
struct DisplayTopologyDisplayView {
    index: u32,
    label: String,
    width: u32,
    height: u32,
}

#[derive(serde::Serialize)]
struct GrpcSessionPeerView {
    client_public_key_b64: String,
    last_seen_at: chrono::DateTime<chrono::Utc>,
    last_peer_ip: String,
    last_grpc_method: String,
    online: bool,
    connection_kind: i32,
    last_cpu_percent: Option<f64>,
    last_ram_percent: Option<f64>,
    last_gpu_percent: Option<f64>,
    proxy_bytes_in_total: u64,
    proxy_bytes_out_total: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    display_topology: Vec<DisplayTopologyDisplayView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_stream_capable: Option<bool>,
}

#[derive(serde::Serialize)]
struct ServerBenchmarkView {
    run_at: chrono::DateTime<chrono::Utc>,
    cpu_score: i32,
    ram_score: i32,
    storage_score: i32,
    gpu_score: Option<i32>,
}

#[derive(serde::Serialize)]
struct GrpcSessionsPageView {
    summary: GrpcSessionsSummaryView,
    /// One row per client key: last activity from the metadata DB (gRPC-oriented).
    peers: Vec<GrpcSessionPeerView>,
    recent: Vec<GrpcSessionEventView>,
    /// Latest `deploy-server resource-benchmark` row for this metadata DB (if any).
    server_benchmark: Option<ServerBenchmarkView>,
}

async fn api_grpc_sessions(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<GrpcSessionsQuery>,
) -> Result<Json<GrpcSessionsPageView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let Some(db) = s.plane.db.as_ref() else {
        return Err(ApiError::service_unavailable(
            "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)",
        ));
    };
    let lim = q.limit.unwrap_or(100).clamp(1, 500);
    let online_secs = q.online_secs.unwrap_or(120).clamp(10, 86_400);
    let total = db
        .count_grpc_session_events_total()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let kind_rows = db
        .fetch_grpc_session_kind_counts()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut by_kind: HashMap<String, i64> = HashMap::new();
    for row in kind_rows {
        by_kind.insert(row.kind, row.event_count);
    }
    let tcp_open_total = *by_kind.get("tcp_open").unwrap_or(&0);
    let tcp_close_total = *by_kind.get("tcp_close").unwrap_or(&0);
    let estimated_open_tcp = (tcp_open_total - tcp_close_total).max(0);
    let peer_rows = db
        .fetch_grpc_session_peer_last_activity()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let now = chrono::Utc::now();
    let mut peers: Vec<GrpcSessionPeerView> = Vec::new();
    for r in peer_rows {
        let kind = db
            .fetch_grpc_peer_profile_kind(&r.client_pubkey_b64)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let snap = db
            .fetch_grpc_peer_resource_snapshot(&r.client_pubkey_b64)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let (bin, bout) = db
            .sum_grpc_proxy_traffic_totals(&r.client_pubkey_b64)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let online = (now - r.last_created_at).num_seconds() <= online_secs;
        let (display_topology, display_stream_capable) = match db
            .fetch_peer_display_topology(&r.client_pubkey_b64)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
        {
            None => (vec![], None),
            Some((_ts, cap, json)) => {
                #[derive(serde::Deserialize)]
                struct Dj {
                    index: u32,
                    label: String,
                    width: u32,
                    height: u32,
                }
                let rows: Vec<Dj> = serde_json::from_str(&json).unwrap_or_default();
                let dt: Vec<DisplayTopologyDisplayView> = rows
                    .into_iter()
                    .map(|x| DisplayTopologyDisplayView {
                        index: x.index,
                        label: x.label,
                        width: x.width,
                        height: x.height,
                    })
                    .collect();
                (dt, Some(cap))
            }
        };
        peers.push(GrpcSessionPeerView {
            client_public_key_b64: r.client_pubkey_b64,
            last_seen_at: r.last_created_at,
            last_peer_ip: r.last_peer_ip,
            last_grpc_method: r.last_grpc_method,
            online,
            connection_kind: kind.unwrap_or(0) as i32,
            last_cpu_percent: snap.as_ref().and_then(|s| s.cpu_percent),
            last_ram_percent: snap.as_ref().and_then(|s| s.ram_percent),
            last_gpu_percent: snap.as_ref().and_then(|s| s.gpu_percent),
            proxy_bytes_in_total: bin,
            proxy_bytes_out_total: bout,
            display_topology,
            display_stream_capable,
        });
    }
    let server_benchmark = db
        .fetch_latest_server_resource_benchmark()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map(|b| ServerBenchmarkView {
            run_at: b.run_at,
            cpu_score: b.cpu_score,
            ram_score: b.ram_score,
            storage_score: b.storage_score,
            gpu_score: b.gpu_score,
        });
    let recent_rows = if q.include_tcp_audit {
        db.fetch_grpc_session_events_page(0, lim)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
    } else {
        db.fetch_grpc_session_events_page_no_tcp(0, lim)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
    };
    let recent: Vec<GrpcSessionEventView> = recent_rows
        .into_iter()
        .map(|r| GrpcSessionEventView {
            id: r.id,
            created_at: r.created_at,
            kind: r.kind,
            peer_ip: r.peer_ip,
            status: r.status,
            grpc_method: r.grpc_method,
            client_public_key_b64: r.client_pubkey_b64,
            detail: r.detail,
        })
        .collect();
    Ok(Json(GrpcSessionsPageView {
        summary: GrpcSessionsSummaryView {
            total_events: total,
            tcp_open_total,
            tcp_close_total,
            estimated_open_tcp,
            closed_tcp_events: tcp_close_total,
            by_kind,
        },
        peers,
        recent,
        server_benchmark,
    }))
}

async fn api_rollback(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<RollbackBody>,
) -> Result<Json<RollbackView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let version = body.version.trim().to_string();
    if version.is_empty() {
        return Err(ApiError::bad_request("version must not be empty"));
    }
    let project_id = project_or_default(&body.project_id);
    s.plane
        .rollback(version, project_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn api_process_stop(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<ProcessControlView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .stop_process(project_or_default(&q.project))
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn api_process_restart(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<ProcessControlView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .restart_process(project_or_default(&q.project))
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(serde::Deserialize)]
struct AppEnvPutBody {
    content: String,
}

async fn api_app_env_get(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<AppEnvView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .read_app_env(&project_or_default(&q.project))
        .map(Json)
        .map_err(Into::into)
}

async fn api_app_env_put(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
    Json(body): Json<AppEnvPutBody>,
) -> Result<Json<AppEnvView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .write_app_env(&project_or_default(&q.project), &body.content)
        .map(Json)
        .map_err(Into::into)
}

fn host_deploy_env_example_template() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../deploy/ubuntu/env.example"
    ))
}

async fn api_host_deploy_env_get(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<HostDeployEnvView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    ControlPlane::read_host_deploy_env(&s.host_deploy_env_path)
        .map(Json)
        .map_err(Into::into)
}

async fn api_host_deploy_env_put(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<AppEnvPutBody>,
) -> Result<Json<HostDeployEnvPutView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    ControlPlane::write_host_deploy_env(
        &s.host_deploy_env_path,
        &body.content,
        &s.host_deploy_env_write_script,
    )
    .map(Json)
    .map_err(Into::into)
}

#[derive(serde::Serialize)]
struct HostDeployEnvTemplateBody {
    template: &'static str,
}

async fn api_host_deploy_env_template(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<HostDeployEnvTemplateBody>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(HostDeployEnvTemplateBody {
        template: host_deploy_env_example_template(),
    }))
}

async fn get_nginx_config(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<NginxConfigView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let Some(ref path) = s.nginx_config_path else {
        return Ok(Json(NginxConfigView {
            path: String::new(),
            content: String::new(),
            enabled: false,
        }));
    };
    read_nginx_config(path)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

async fn put_nginx_config(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<NginxConfigPut>,
) -> Result<Json<NginxPutResponseView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    match &s.nginx_config_path {
        None => Err(ApiError::service_unavailable(
            "NGINX_CONFIG_PATH is not set",
        )),
        Some(path) => {
            check_nginx_write_auth(&s, &headers)?;
            let outcome = apply_nginx_put(path, &body.content, s.nginx_test_full_config)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            Ok(Json(outcome.response))
        }
    }
}

async fn api_nginx_status(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<NginxStatusView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(collect_nginx_status(
        &s.nginx_site_path,
        &s.nginx_ensure_script,
        &s.nginx_apply_site_script,
    )))
}

async fn api_nginx_site_get(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<NginxConfigView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(read_nginx_site_file(&s.nginx_site_path)))
}

async fn api_nginx_site_put(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<NginxConfigPut>,
) -> Result<Json<NginxPutResponseView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let out = apply_nginx_site_via_sudo(
        &s.nginx_site_path,
        &body.content,
        &s.nginx_apply_site_script,
    )?;
    Ok(Json(out))
}

#[derive(serde::Deserialize)]
struct NginxEnsureBody {
    mode: String,
}

async fn api_host_services(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<HostServicesView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(collect_host_services(
        &s.nginx_site_path,
        &s.nginx_ensure_script,
        &s.nginx_apply_site_script,
        &s.host_service_dispatch_script,
    )))
}

async fn api_host_service_install(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<HostServiceActionView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(host_service_action_via_sudo(
        "install",
        &id,
        &s.host_service_dispatch_script,
    )?))
}

async fn api_host_service_remove(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<HostServiceActionView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(host_service_action_via_sudo(
        "remove",
        &id,
        &s.host_service_dispatch_script,
    )?))
}

async fn api_nginx_ensure(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<NginxEnsureBody>,
) -> Result<Json<NginxEnsureView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let mode = body.mode.trim().to_string();
    let mut out = ensure_nginx_via_sudo(&mode, &s.nginx_ensure_script)?;
    if !out.ok {
        return Ok(Json(out));
    }
    let current_env = ControlPlane::read_host_deploy_env(&s.host_deploy_env_path)?;
    let (next_content, updates) =
        apply_nginx_env_sync(&current_env.content, &mode, s.control_api_port)?;
    let env_put = ControlPlane::write_host_deploy_env(
        &s.host_deploy_env_path,
        &next_content,
        &s.host_deploy_env_write_script,
    )?;
    out.env_update = Some(NginxEnvUpdateView {
        mode,
        restart_scheduled: env_put.restart_scheduled,
        updates,
    });
    Ok(Json(out))
}

#[derive(Parser, Debug)]
#[command(
    name = "control-api",
    about = "HTTP API for deploy dashboard",
    subcommand_required = false
)]
struct Top {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    serve: Args,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create Ed25519 identity for control-api and register its public key in deploy-server `authorized_peers.json`.
    #[command(name = "bootstrap-grpc-key")]
    BootstrapGrpcKey(BootstrapArgs),
}

#[derive(Parser, Debug)]
struct BootstrapArgs {
    /// Overwrite existing key file and replace its public key in `authorized_peers`.
    #[arg(long, default_value_t = false)]
    force: bool,
    /// Path for identity JSON (default: `<DEPLOY_ROOT>/.keys/control_api_ed25519.json`).
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Parser, Debug)]
struct Args {
    /// Same `--root` as deploy-server (for listing `releases/`).
    #[arg(long, default_value = "/deploy", env = "DEPLOY_ROOT")]
    deploy_root: PathBuf,

    /// deploy-server gRPC HTTP/2 endpoint (IPv6).
    #[arg(long, default_value = "http://[::1]:50051", env = "GRPC_ENDPOINT")]
    grpc_endpoint: String,

    /// Bind address (`127.0.0.1` / `::1` for localhost-only; `::` for all interfaces).
    #[arg(long, env = "CONTROL_API_BIND", default_value = "::")]
    bind: IpAddr,

    /// HTTP listen port.
    #[arg(short, long, default_value_t = 8080, env = "CONTROL_API_PORT")]
    listen_port: u16,

    /// Metadata SQLite URL (native install). Takes precedence over `DATABASE_URL` when both are set.
    #[arg(long, env = "DEPLOY_SQLITE_URL")]
    deploy_sqlite_url: Option<String>,

    /// Metadata database URL: PostgreSQL in Docker, or omit when using `DEPLOY_SQLITE_URL` only.
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Optional PostgreSQL for dashboard explorer + database-info when metadata is SQLite-only.
    #[arg(long, env = "POSTGRES_EXPLORER_URL")]
    postgres_explorer_url: Option<String>,

    /// Path to nginx config file for GET/PUT `/api/v1/nginx/config` (optional).
    #[arg(long, env = "NGINX_CONFIG_PATH")]
    nginx_config_path: Option<PathBuf>,

    /// If true, after save run `nginx -t -c <NGINX_CONFIG_PATH>`; if false, run `nginx -t` (validates default main config tree).
    #[arg(long, default_value_t = false, env = "NGINX_TEST_FULL_CONFIG")]
    nginx_test_full_config: bool,

    /// If set, PUT `/api/v1/nginx/config` requires `Authorization: Bearer <token>`.
    #[arg(long, env = "NGINX_ADMIN_TOKEN")]
    nginx_admin_token: Option<String>,

    /// Path to nginx site config managed by desktop tab (`GET/PUT /api/v1/nginx/site`).
    #[arg(
        long,
        env = "CONTROL_API_NGINX_SITE_PATH",
        default_value = "/etc/nginx/sites-available/pirate"
    )]
    nginx_site_path: PathBuf,

    /// Privileged helper to install/start nginx (`POST /api/v1/nginx/ensure`).
    #[arg(
        long,
        env = "CONTROL_API_NGINX_ENSURE_SCRIPT",
        default_value = "/usr/local/lib/pirate/pirate-ensure-nginx.sh"
    )]
    nginx_ensure_script: PathBuf,

    /// Privileged helper to apply nginx site from stdin (`PUT /api/v1/nginx/site`).
    #[arg(
        long,
        env = "CONTROL_API_NGINX_APPLY_SITE_SCRIPT",
        default_value = "/usr/local/lib/pirate/pirate-nginx-apply-site.sh"
    )]
    nginx_apply_site_script: PathBuf,

    /// `pirate-host-service.sh` whitelist (install/remove optional host packages).
    #[arg(
        long,
        env = "CONTROL_API_HOST_SERVICE_DISPATCH_SCRIPT",
        default_value = "/usr/local/lib/pirate/pirate-host-service.sh"
    )]
    host_service_dispatch_script: PathBuf,

    /// Anti-DDoS state directory (`host.json`, `projects/*.json`).
    #[arg(
        long,
        env = "CONTROL_API_ANTIDDOS_STATE_DIR",
        default_value = "/var/lib/pirate/antiddos"
    )]
    antiddos_state_dir: PathBuf,

    /// Privileged helper: `pirate-antiddos-apply.sh`.
    #[arg(
        long,
        env = "CONTROL_API_ANTIDDOS_APPLY_SCRIPT",
        default_value = "/usr/local/lib/pirate/pirate-antiddos-apply.sh"
    )]
    antiddos_apply_script: PathBuf,

    /// Nginx log for rate-limit events (fail2ban + stats tail).
    #[arg(
        long,
        env = "CONTROL_API_ANTIDDOS_LIMIT_LOG",
        default_value = "/var/log/nginx/pirate-antiddos-error.log"
    )]
    antiddos_limit_log_path: PathBuf,

    /// If set, `Authorization: Bearer` may match this token (in addition to JWT when `CONTROL_API_JWT_SECRET` is set).
    #[arg(long, env = "CONTROL_API_BEARER_TOKEN")]
    api_bearer_token: Option<String>,

    /// HS256 secret for JWTs from `/api/v1/auth/login`. When set with a metadata DB URL, dashboard login is enabled.
    #[arg(long, env = "CONTROL_API_JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Lifetime for issued JWTs (seconds).
    #[arg(long, default_value_t = 28800, env = "CONTROL_API_JWT_TTL_SECS")]
    jwt_ttl_secs: u64,

    /// Max project artifact size for HTTP multipart deploy (must match deploy-server). Env: `DEPLOY_MAX_UPLOAD_BYTES`.
    #[arg(long, env = "DEPLOY_MAX_UPLOAD_BYTES", default_value_t = 256 * 1024 * 1024)]
    max_artifact_upload_bytes: u64,

    /// Chunk size when forwarding HTTP-uploaded tarball to gRPC (bytes).
    #[arg(long, env = "CONTROL_API_DEPLOY_CHUNK_BYTES", default_value_t = 256 * 1024)]
    deploy_http_chunk_bytes: usize,

    /// Suggested chunk size for resumable deploy upload sessions (bytes).
    #[arg(
        long,
        env = "CONTROL_API_DEPLOY_SESSION_CHUNK_BYTES",
        default_value_t = 1024 * 1024
    )]
    deploy_upload_session_chunk_bytes: usize,

    /// TTL for resumable deploy upload sessions (seconds).
    #[arg(
        long,
        env = "CONTROL_API_DEPLOY_SESSION_TTL_SECS",
        default_value_t = 3600
    )]
    deploy_upload_session_ttl_secs: u64,

    /// Ed25519 identity JSON for signed gRPC `GetStatus` to deploy-server (when server enforces auth).
    #[arg(long, env = "GRPC_SIGNING_KEY_PATH")]
    grpc_signing_key_path: Option<PathBuf>,

    /// If set, last lines of this file are included in `GET /api/v1/host-stats` as `log_tail`.
    #[arg(long, env = "CONTROL_API_LOG_TAIL_PATH")]
    log_tail_path: Option<PathBuf>,

    /// Set to `1` to maintain in-memory series for `GET /api/v1/host-stats/series`.
    #[arg(long, env = "CONTROL_API_HOST_STATS_SERIES", default_value = "0")]
    host_stats_series: u8,

    /// Set to `1` to enable `GET /api/v1/host-stats/stream` (SSE) and WebSocket `/api/v1/host-stats/ws`.
    #[arg(long, env = "CONTROL_API_HOST_STATS_STREAM", default_value = "0")]
    host_stats_stream: u8,

    /// Set to `1` to enable WebSocket interactive shell at `/api/v1/host-terminal/ws` (Unix; runs as the control-api OS user).
    #[arg(long, env = "CONTROL_API_HOST_TERMINAL", default_value = "0")]
    host_terminal: u8,

    /// Shell for host terminal (absolute path). Env: `CONTROL_API_HOST_TERMINAL_SHELL`.
    #[arg(
        long,
        env = "CONTROL_API_HOST_TERMINAL_SHELL",
        default_value = "/bin/bash"
    )]
    host_terminal_shell: PathBuf,

    /// Max WebSocket host-terminal session duration (seconds).
    #[arg(long, env = "CONTROL_API_HOST_TERMINAL_SESSION_SECS", default_value_t = 3600)]
    host_terminal_session_secs: u64,

    /// Root for credential files (PostgreSQL, etc.) and per-id dirs if server-side SMB is configured.
    /// `install.sh` creates `/var/lib/pirate/db-mounts` (SMB mounts themselves use Pirate Client, not the server).
    #[arg(long, env = "PIRATE_DATA_MOUNTS_ROOT", default_value = "/var/lib/pirate/db-mounts")]
    data_mounts_root: PathBuf,

    /// Optional: only if you add mount helpers on the host; not installed by `install.sh`.
    #[arg(
        long,
        env = "PIRATE_SMB_MOUNT_SCRIPT",
        default_value = "/usr/local/lib/pirate/pirate-smb-mount.sh"
    )]
    smb_mount_script: PathBuf,

    #[arg(
        long,
        env = "PIRATE_SMB_UMOUNT_SCRIPT",
        default_value = "/usr/local/lib/pirate/pirate-smb-umount.sh"
    )]
    smb_umount_script: PathBuf,
}

fn spawn_reconcile(plane: Arc<ControlPlane>) {
    let Some(db) = plane.db.clone() else {
        return;
    };
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = plane.reconcile_snapshot(&db).await {
                warn!(%e, "reconcile upsert_snapshot");
            }
        }
    });
}

fn deploy_root_from_env() -> PathBuf {
    std::env::var("DEPLOY_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/pirate/deploy"))
}

/// Create `control_api_ed25519.json` and ensure its public key is in `authorized_peers.json`
/// under `<DEPLOY_ROOT>/.keys/` (deploy-server must have started once).
fn bootstrap_grpc_key(args: BootstrapArgs) -> Result<(), Box<dyn std::error::Error>> {
    let deploy_root = deploy_root_from_env();
    let keys_dir = deploy_root.join(".keys");
    if !keys_dir.is_dir() {
        return Err(format!(
            "missing keys directory {} — start deploy-server at least once first",
            keys_dir.display()
        )
        .into());
    }
    let key_path = args
        .output
        .unwrap_or_else(|| keys_dir.join("control_api_ed25519.json"));
    let peers_path = keys_dir.join("authorized_peers.json");

    let sk = if key_path.exists() {
        if args.force {
            let old = load_identity(&key_path)?;
            let mut peers = load_authorized_peers(&peers_path)?;
            peers.remove(old.verifying_key().as_bytes());
            save_authorized_peers(&peers_path, &peers)?;
            let id = IdentityFile::generate();
            std::fs::write(&key_path, serde_json::to_string_pretty(&id)?)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
            }
            id.to_signing_key()?
        } else {
            load_identity(&key_path)?
        }
    } else {
        let id = IdentityFile::generate();
        std::fs::write(&key_path, serde_json::to_string_pretty(&id)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        id.to_signing_key()?
    };

    let mut peers = load_authorized_peers(&peers_path)?;
    peers.insert(*sk.verifying_key().as_bytes());
    save_authorized_peers(&peers_path, &peers)?;
    info!(
        key = %key_path.display(),
        "bootstrap-grpc-key: control-api gRPC identity ready"
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let top = Top::parse();
    match top.command {
        Some(Commands::BootstrapGrpcKey(b)) => bootstrap_grpc_key(b),
        None => run_serve(top.serve).await,
    }
}

async fn run_serve(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        root = %args.deploy_root.display(),
        grpc = %args.grpc_endpoint,
        max_artifact_upload_bytes = args.max_artifact_upload_bytes,
        port = args.listen_port,
        nginx_config = ?args.nginx_config_path,
        nginx_test_full = args.nginx_test_full_config,
        nginx_auth = %if args.nginx_admin_token.is_some() { "on" } else { "off" },
        nginx_site_path = %args.nginx_site_path.display(),
        nginx_ensure_script = %args.nginx_ensure_script.display(),
        nginx_apply_site_script = %args.nginx_apply_site_script.display(),
        api_bearer = %if args.api_bearer_token.is_some() { "on" } else { "off" },
        jwt = %if args.jwt_secret.is_some() { "on" } else { "off" },
        host_terminal = %if args.host_terminal != 0 { "on" } else { "off" },
        cors = %if std::env::var("CONTROL_API_CORS_ALLOW_ANY").ok().as_deref() == Some("1") {
            "allow_any"
        } else {
            "restricted"
        },
        "starting control-api"
    );

    let metadata_url = args
        .deploy_sqlite_url
        .clone()
        .or_else(|| args.database_url.clone())
        .filter(|s| !s.trim().is_empty());

    let db = if let Some(ref url) = metadata_url {
        let store = DbStore::connect(url).await?;
        info!("metadata database connected (control-api); migrations are applied by deploy-server only");
        Some(Arc::new(store))
    } else {
        None
    };

    let pg_explorer: Option<Arc<PgPool>> =
        if let Some(ref u) = args.postgres_explorer_url {
            if !u.trim().is_empty() {
                let pool = PgPoolOptions::new()
                    .max_connections(3)
                    .connect(u)
                    .await?;
                info!("PostgreSQL explorer pool connected");
                Some(Arc::new(pool))
            } else {
                None
            }
        } else if let Some(ref d) = db {
            d.pg_pool().map(|p| Arc::new(p.clone()))
        } else {
            None
        };

    let grpc_signing_key: Option<Arc<SigningKey>> = match &args.grpc_signing_key_path {
        Some(p) => Some(Arc::new(load_identity(p)?)),
        None => None,
    };

    let plane = Arc::new(ControlPlane::new(
        args.deploy_root.clone(),
        args.grpc_endpoint.clone(),
        db.clone(),
        pg_explorer,
        grpc_signing_key,
    ));

    if db.is_some() {
        spawn_reconcile(plane.clone());
    }

    let host_stats_series_enabled = args.host_stats_series != 0;
    let host_stats_stream_enabled = args.host_stats_stream != 0;
    let host_terminal_enabled = args.host_terminal != 0;
    let host_history = if host_stats_series_enabled {
        Some(Arc::new(Mutex::new(HostStatsHistory::default_new())))
    } else {
        None
    };

    let explorer_connection_display = args
        .postgres_explorer_url
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|u| redact_database_url(u))
        .or_else(|| {
            if db.as_ref().map(|d| d.is_postgres()).unwrap_or(false) {
                args.database_url
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .and_then(|u| redact_database_url(u))
            } else {
                None
            }
        });

    let tunnel_redis = std::env::var("DEPLOY_REDIS_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .and_then(|url| redis::Client::open(url).ok());

    let subscription_public_base = std::env::var("DEPLOY_SUBSCRIPTION_PUBLIC_HOST")
        .or_else(|_| std::env::var("CONTROL_API_SUBSCRIPTION_PUBLIC_HOST"))
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    let subscription_server_hostname = subscription_public_base.as_deref().and_then(|raw| {
        if raw.contains("://") {
            url::Url::parse(raw)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()))
        } else {
            raw.split('/')
                .next()
                .and_then(|h| h.split(':').next())
                .map(|s| s.to_string())
        }
    });
    let subscription_tls_sni = std::env::var("DEPLOY_SUBSCRIPTION_TLS_SNI")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let grpc_public_url = std::env::var("DEPLOY_GRPC_PUBLIC_URL")
        .or_else(|_| std::env::var("CONTROL_API_GRPC_PUBLIC_URL"))
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());

    let host_deploy_env_path = std::env::var("CONTROL_API_HOST_DEPLOY_ENV_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/pirate-deploy.env"));
    let host_deploy_env_write_script = std::env::var("CONTROL_API_WRITE_DEPLOY_ENV_SCRIPT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/lib/pirate/pirate-write-deploy-env.sh"));

    let state = ApiState {
        plane,
        nginx_config_path: args.nginx_config_path.clone(),
        nginx_test_full_config: args.nginx_test_full_config,
        nginx_admin_token: args.nginx_admin_token.clone(),
        api_bearer_token: args.api_bearer_token.clone(),
        jwt_secret: args.jwt_secret.clone(),
        jwt_ttl_secs: args.jwt_ttl_secs,
        host_net: Arc::new(Mutex::new(None)),
        log_tail_path: args.log_tail_path.clone(),
        host_stats_series_enabled,
        host_history,
        host_stats_stream_enabled,
        host_terminal_enabled,
        host_terminal_shell: args.host_terminal_shell.clone(),
        host_terminal_session_secs: args.host_terminal_session_secs,
        explorer_connection_display,
        data_mounts_root: args.data_mounts_root.clone(),
        smb_mount_script: args.smb_mount_script.clone(),
        smb_umount_script: args.smb_umount_script.clone(),
        tunnel_redis,
        subscription_public_base,
        subscription_server_hostname,
        subscription_tls_sni,
        grpc_public_url,
        host_deploy_env_path,
        host_deploy_env_write_script,
        control_api_port: args.listen_port,
        nginx_site_path: args.nginx_site_path.clone(),
        nginx_ensure_script: args.nginx_ensure_script.clone(),
        nginx_apply_site_script: args.nginx_apply_site_script.clone(),
        host_service_dispatch_script: args.host_service_dispatch_script.clone(),
        antiddos_state_dir: args.antiddos_state_dir.clone(),
        antiddos_apply_script: args.antiddos_apply_script.clone(),
        antiddos_limit_log_path: args.antiddos_limit_log_path.clone(),
        max_artifact_upload_bytes_configured: args.max_artifact_upload_bytes,
        deploy_http_chunk_bytes: args.deploy_http_chunk_bytes,
        deploy_upload_session_chunk_bytes: args.deploy_upload_session_chunk_bytes,
        deploy_upload_session_ttl_secs: args.deploy_upload_session_ttl_secs,
        deploy_upload_sessions: Arc::new(AsyncMutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/ping", get(api_ping))
        .route("/api/v1/auth/login", post(api_login))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/projects/telemetry", get(api_project_telemetry))
        .route(
            "/api/v1/projects/telemetry/clear",
            post(api_project_telemetry_clear),
        )
        .route("/api/v1/host-stats", get(api_host_stats))
        .route(
            "/api/v1/host-stats/detail/cpu",
            get(api_host_stats_detail_cpu),
        )
        .route(
            "/api/v1/host-stats/detail/memory",
            get(api_host_stats_detail_memory),
        )
        .route(
            "/api/v1/host-stats/detail/disk",
            get(api_host_stats_detail_disk),
        )
        .route(
            "/api/v1/host-stats/detail/network",
            get(api_host_stats_detail_network),
        )
        .route(
            "/api/v1/host-stats/detail/processes",
            get(api_host_stats_detail_processes),
        )
        .route("/api/v1/host-stats/series", get(api_host_stats_series))
        .route("/api/v1/host-stats/stream", get(api_host_stats_sse))
        .route("/api/v1/host-stats/ws", get(api_host_stats_ws))
        .route(
            "/api/v1/host-terminal/ws",
            get(host_terminal::api_host_terminal_ws),
        )
        .route("/api/v1/releases", get(api_releases))
        .route("/api/v1/projects", get(api_projects))
        .route("/api/v1/projects/allocate", post(api_projects_allocate))
        // Register session routes before bare `deploy-artifact` (longer, static `session` path).
        .route(
            "/api/v1/projects/:project_id/deploy-artifact/session",
            post(api_deploy_artifact_session_create),
        )
        .route(
            "/api/v1/projects/:project_id/deploy-artifact/session/:upload_id/chunk",
            put(api_deploy_artifact_session_chunk),
        )
        .route(
            "/api/v1/projects/:project_id/deploy-artifact/session/:upload_id/complete",
            post(api_deploy_artifact_session_complete),
        )
        .route(
            "/api/v1/projects/:project_id/deploy-artifact/session/:upload_id",
            delete(api_deploy_artifact_session_delete),
        )
        .route(
            "/api/v1/projects/:project_id/deploy-artifact",
            post(api_deploy_artifact_multipart),
        )
        .route("/api/v1/history", get(api_history))
        .route("/api/v1/grpc-sessions", get(api_grpc_sessions))
        .route(
            "/api/v1/proxy-sessions",
            get(proxy_sessions_api::api_proxy_sessions_list)
                .post(proxy_sessions_api::api_proxy_sessions_create),
        )
        .route(
            "/api/v1/proxy-sessions/:id",
            patch(proxy_sessions_api::api_proxy_sessions_patch),
        )
        .route(
            "/api/v1/proxy-sessions/:id/revoke",
            post(proxy_sessions_api::api_proxy_sessions_revoke),
        )
        .route(
            "/api/v1/proxy-sessions/:id/xray-config",
            get(proxy_sessions_api::api_proxy_session_xray_config),
        )
        .route(
            "/api/v1/public/proxy-subscription/:token",
            get(proxy_sessions_api::api_public_proxy_subscription),
        )
        .route(
            "/api/v1/public/pirate-bootstrap/:token",
            get(proxy_sessions_api::api_public_pirate_bootstrap),
        )
        .route("/api/v1/bootstrap-hints", get(api_bootstrap_hints))
        .route("/api/v1/database-info", get(api_database_info))
        .route("/api/v1/database/schemas", get(api_database_schemas))
        .route("/api/v1/database/tables", get(api_database_tables))
        .route(
            "/api/v1/database/tables/:schema/:table/columns",
            get(api_database_columns),
        )
        .route(
            "/api/v1/database/tables/:schema/:table/rows",
            get(api_database_table_rows),
        )
        .route(
            "/api/v1/database/relationships",
            get(api_database_relationships),
        )
        .route(
            "/api/v1/data-sources",
            get(data_sources_api::api_data_sources_list),
        )
        .route(
            "/api/v1/data-sources/smb",
            post(data_sources_api::api_post_smb),
        )
        .route(
            "/api/v1/data-sources/connection",
            post(data_sources_api::api_post_connection),
        )
        .route(
            "/api/v1/data-sources/:id",
            delete(data_sources_api::api_data_sources_delete),
        )
        .route(
            "/api/v1/data-sources/:id/browse",
            get(data_sources_api::api_smb_browse),
        )
        .route("/api/v1/rollback", post(api_rollback))
        .route("/api/v1/process/stop", post(api_process_stop))
        .route("/api/v1/process/restart", post(api_process_restart))
        .route(
            "/api/v1/app-env",
            get(api_app_env_get).put(api_app_env_put),
        )
        .route(
            "/api/v1/host-deploy-env",
            get(api_host_deploy_env_get).put(api_host_deploy_env_put),
        )
        .route(
            "/api/v1/host-deploy-env/template",
            get(api_host_deploy_env_template),
        )
        .route("/api/v1/nginx/config", get(get_nginx_config))
        .route("/api/v1/nginx/config", put(put_nginx_config))
        .route("/api/v1/nginx/status", get(api_nginx_status))
        .route("/api/v1/nginx/site", get(api_nginx_site_get).put(api_nginx_site_put))
        .route("/api/v1/nginx/ensure", post(api_nginx_ensure))
        .route("/api/v1/host-services", get(api_host_services))
        .route(
            "/api/v1/host-services/:id/install",
            post(api_host_service_install),
        )
        .route(
            "/api/v1/host-services/:id/remove",
            post(api_host_service_remove),
        )
        .route("/api/v1/antiddos", get(antiddos_api::api_antiddos_get).put(antiddos_api::api_antiddos_put))
        .route(
            "/api/v1/antiddos/enable",
            post(antiddos_api::api_antiddos_enable),
        )
        .route(
            "/api/v1/antiddos/disable",
            post(antiddos_api::api_antiddos_disable),
        )
        .route(
            "/api/v1/antiddos/apply",
            post(antiddos_api::api_antiddos_apply),
        )
        .route("/api/v1/antiddos/stats", get(antiddos_api::api_antiddos_stats))
        .route(
            "/api/v1/antiddos/projects/:project_id",
            put(antiddos_api::api_antiddos_project_put)
                .delete(antiddos_api::api_antiddos_project_delete),
        )
        .layer(DefaultBodyLimit::max(
            (args.max_artifact_upload_bytes as usize)
                .saturating_add(64 * 1024 * 1024)
                .max(10 * 1024 * 1024),
        ))
        .layer(cors::build_cors_layer())
        .with_state(state);

    let addr = SocketAddr::new(args.bind, args.listen_port);
    let listener = TcpListener::bind(addr).await?;
    info!(listen = %addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod upload_limit_tests {
    use super::effective_upload_limit_from_config_and_grpc;

    #[test]
    fn effective_min_when_grpc_smaller() {
        assert_eq!(
            effective_upload_limit_from_config_and_grpc(256 * 1024 * 1024, Some(100_000)),
            100_000
        );
    }

    #[test]
    fn effective_min_when_config_smaller() {
        assert_eq!(
            effective_upload_limit_from_config_and_grpc(100_000, Some(256 * 1024 * 1024)),
            100_000
        );
    }

    #[test]
    fn effective_uses_config_when_grpc_none() {
        assert_eq!(
            effective_upload_limit_from_config_and_grpc(100_000, None),
            100_000
        );
    }

    #[test]
    fn effective_uses_config_when_grpc_zero() {
        assert_eq!(
            effective_upload_limit_from_config_and_grpc(100_000, Some(0)),
            100_000
        );
    }
}
