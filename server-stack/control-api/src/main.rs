//! HTTP control plane: status (via gRPC to deploy-server), releases (FS), history (PostgreSQL), nginx config.

mod cors;
mod error;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderMap;
use axum::routing::{get, put};
use axum::{Json, Router};
use clap::Parser;
use deploy_control::{
    apply_nginx_put, read_nginx_config, ControlPlane, NginxConfigPut, NginxConfigView,
    NginxPutResponseView,
};
use deploy_db::DbStore;
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
    /// When set, all `/api/v1/*` routes require `Authorization: Bearer <token>`.
    api_bearer_token: Option<String>,
}

fn check_api_bearer(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    match &state.api_bearer_token {
        None => Ok(()),
        Some(tok) => {
            let ok = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|a| a == format!("Bearer {}", tok));
            if ok {
                Ok(())
            } else {
                Err(ApiError::unauthorized(
                    "missing or invalid Authorization Bearer token",
                ))
            }
        }
    }
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

async fn api_status(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<deploy_control::StatusView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane.get_status().await.map(Json).map_err(Into::into)
}

async fn api_releases(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<deploy_control::ReleasesView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane.list_releases().map(Json).map_err(Into::into)
}

async fn api_history(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<deploy_control::HistoryView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    s.plane
        .fetch_history(100)
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
#[command(name = "control-api", about = "HTTP API for deploy dashboard")]
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

    /// If set, all `/api/v1/*` routes require `Authorization: Bearer <token>`.
    #[arg(long, env = "CONTROL_API_BEARER_TOKEN")]
    api_bearer_token: Option<String>,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    info!(
        root = %args.deploy_root.display(),
        grpc = %args.grpc_endpoint,
        port = args.listen_port,
        nginx_config = ?args.nginx_config_path,
        nginx_test_full = args.nginx_test_full_config,
        nginx_auth = %if args.nginx_admin_token.is_some() { "on" } else { "off" },
        api_bearer = %if args.api_bearer_token.is_some() { "on" } else { "off" },
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

    let plane = Arc::new(ControlPlane::new(
        args.deploy_root.clone(),
        args.grpc_endpoint.clone(),
        db.clone(),
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
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/releases", get(api_releases))
        .route("/api/v1/history", get(api_history))
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
