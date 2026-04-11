//! HTTP control plane: status (via gRPC to deploy-server), releases (FS), history (PostgreSQL), nginx config.

mod auth;
mod cors;
mod error;

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use deploy_auth::{
    load_authorized_peers, load_identity, save_authorized_peers, IdentityFile,
};
use deploy_control::{
    apply_nginx_put, read_nginx_config, ControlPlane, NginxConfigPut, NginxConfigView,
    NginxPutResponseView, ProcessControlView, ProjectsView, RollbackBody, RollbackView,
};
use deploy_db::DbStore;
use ed25519_dalek::SigningKey;
use error::ApiError;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
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
}

fn bearer_raw(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
}

fn check_api_bearer(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let require_auth = state.api_bearer_token.is_some() || state.jwt_secret.is_some();
    if !require_auth {
        return Ok(());
    }
    let Some(token) = bearer_raw(headers) else {
        return Err(ApiError::unauthorized(
            "missing or invalid Authorization Bearer token",
        ));
    };
    if let Some(ref static_tok) = state.api_bearer_token {
        if token == static_tok.as_str() {
            return Ok(());
        }
    }
    if let Some(ref secret) = state.jwt_secret {
        if auth::decode_access_token(token, secret).is_ok() {
            return Ok(());
        }
    }
    Err(ApiError::unauthorized(
        "missing or invalid Authorization Bearer token",
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
            "DATABASE_URL is not configured",
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

    /// Bind address (`::` or `0.0.0.0` for Docker / all interfaces).
    #[arg(long, default_value = "::")]
    bind: IpAddr,

    /// HTTP listen port.
    #[arg(short, long, default_value_t = 8080, env = "CONTROL_API_PORT")]
    listen_port: u16,

    /// Optional; enables `/api/v1/history` and DB fallback for status.
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

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

    /// HS256 secret for JWTs from `/api/v1/auth/login`. When set with `DATABASE_URL`, dashboard login is enabled.
    #[arg(long, env = "CONTROL_API_JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Lifetime for issued JWTs (seconds).
    #[arg(long, default_value_t = 28800, env = "CONTROL_API_JWT_TTL_SECS")]
    jwt_ttl_secs: u64,

    /// Ed25519 identity JSON for signed gRPC `GetStatus` to deploy-server (when server enforces auth).
    #[arg(long, env = "GRPC_SIGNING_KEY_PATH")]
    grpc_signing_key_path: Option<PathBuf>,
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

    let db = if let Some(ref url) = args.database_url {
        let store = DbStore::connect(url).await?;
        info!("PostgreSQL connected (control-api); schema migrations are applied by deploy-server only");
        Some(Arc::new(store))
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
        grpc_signing_key,
    ));

    if db.is_some() {
        spawn_reconcile(plane.clone());
    }

    let state = ApiState {
        plane,
        nginx_config_path: args.nginx_config_path.clone(),
        nginx_test_full_config: args.nginx_test_full_config,
        nginx_admin_token: args.nginx_admin_token.clone(),
        api_bearer_token: args.api_bearer_token.clone(),
        jwt_secret: args.jwt_secret.clone(),
        jwt_ttl_secs: args.jwt_ttl_secs,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/auth/login", post(api_login))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/releases", get(api_releases))
        .route("/api/v1/projects", get(api_projects))
        .route("/api/v1/history", get(api_history))
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
