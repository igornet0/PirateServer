//! HTTP control plane: status (via gRPC to deploy-server), releases (FS), history (PostgreSQL), nginx config.

mod auth;
mod cors;
mod data_sources_api;
mod error;

use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::Stream;
use std::convert::Infallible;
use clap::{Parser, Subcommand};
use deploy_auth::{
    load_authorized_peers, load_identity, save_authorized_peers, IdentityFile,
};
use deploy_control::{
    apply_nginx_put, read_nginx_config, ControlPlane, CpuDetail, DatabaseColumnsView,
    DatabaseInfoView, DatabaseRelationshipsView, DatabaseSchemasView, DatabaseTablePreviewView,
    DatabaseTablesView, DiskDetail, HostStatsHistory, MemoryDetail, NginxConfigPut, NginxConfigView,
    NginxPutResponseView, NetworkDetail, ProcessControlView, ProcessesDetail, ProjectsView,
    RollbackBody, RollbackView, SeriesResponse,
};
use deploy_db::{DbStore, PgPool};
use sqlx::postgres::PgPoolOptions;
use ed25519_dalek::SigningKey;
use error::ApiError;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tracing::{info, warn};

#[derive(Clone)]
struct ApiState {
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
    /// Explorer PostgreSQL URL with password removed (for `GET /api/v1/database-info`).
    explorer_connection_display: Option<String>,
    /// Root for DB credential files and optional server-side SMB dirs (`install.sh` creates the tree).
    data_mounts_root: PathBuf,
    smb_mount_script: PathBuf,
    smb_umount_script: PathBuf,
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
fn check_api_bearer_with_query(
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
struct HistoryQuery {
    /// When omitted or empty, return events for all projects.
    #[serde(default)]
    project: Option<String>,
}

fn project_or_default(project: &str) -> String {
    if project.trim().is_empty() {
        "default".to_string()
    } else {
        project.trim().to_string()
    }
}

async fn api_status(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<deploy_control::StatusView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .get_status(&project_or_default(&q.project))
        .await
        .map(Json)
        .map_err(Into::into)
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
struct StreamAuthQuery {
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

    /// If set, `Authorization: Bearer` may match this token (in addition to JWT when `CONTROL_API_JWT_SECRET` is set).
    #[arg(long, env = "CONTROL_API_BEARER_TOKEN")]
    api_bearer_token: Option<String>,

    /// HS256 secret for JWTs from `/api/v1/auth/login`. When set with a metadata DB URL, dashboard login is enabled.
    #[arg(long, env = "CONTROL_API_JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Lifetime for issued JWTs (seconds).
    #[arg(long, default_value_t = 28800, env = "CONTROL_API_JWT_TTL_SECS")]
    jwt_ttl_secs: u64,

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
        port = args.listen_port,
        nginx_config = ?args.nginx_config_path,
        nginx_test_full = args.nginx_test_full_config,
        nginx_auth = %if args.nginx_admin_token.is_some() { "on" } else { "off" },
        api_bearer = %if args.api_bearer_token.is_some() { "on" } else { "off" },
        jwt = %if args.jwt_secret.is_some() { "on" } else { "off" },
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
        explorer_connection_display,
        data_mounts_root: args.data_mounts_root.clone(),
        smb_mount_script: args.smb_mount_script.clone(),
        smb_umount_script: args.smb_umount_script.clone(),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/auth/login", post(api_login))
        .route("/api/v1/status", get(api_status))
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
        .route("/api/v1/releases", get(api_releases))
        .route("/api/v1/projects", get(api_projects))
        .route("/api/v1/history", get(api_history))
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
        .route("/api/v1/nginx/config", get(get_nginx_config))
        .route("/api/v1/nginx/config", put(put_nginx_config))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(cors::build_cors_layer())
        .with_state(state);

    let addr = SocketAddr::new(args.bind, args.listen_port);
    let listener = TcpListener::bind(addr).await?;
    info!(listen = %addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}
