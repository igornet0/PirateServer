//! Loopback/LAN HTTP: POST JPEG to `/ingest`, GET `/last.jpg` for preview.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use parking_lot::Mutex;
use parking_lot::RwLock;
use tower_http::cors::{Any, CorsLayer};

static DISPLAY_INGEST_PORT: AtomicU16 = AtomicU16::new(0);
static DISPLAY_INGEST_STARTED: AtomicBool = AtomicBool::new(false);
static DISPLAY_INGEST_SHARED: Mutex<Option<Arc<IngestShared>>> = Mutex::new(None);
static DISPLAY_INGEST_SPAWN_LOCK: Mutex<()> = Mutex::new(());

pub struct IngestShared {
    pub token: RwLock<Option<String>>,
    pub last_jpeg: RwLock<Vec<u8>>,
}

impl IngestShared {
    fn new(token: Option<String>) -> Self {
        Self {
            token: RwLock::new(token),
            last_jpeg: RwLock::new(Vec::new()),
        }
    }
}

fn check_token(state: &IngestShared, headers: &HeaderMap) -> bool {
    let expected = state.token.read();
    let Some(ref want) = *expected else {
        return true;
    };
    if want.is_empty() {
        return true;
    }
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let prefix = "Bearer ";
    auth.strip_prefix(prefix).map(str::trim) == Some(want.trim())
}

async fn post_ingest(
    State(state): State<Arc<IngestShared>>,
    headers: HeaderMap,
    body: Body,
) -> impl IntoResponse {
    if !check_token(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let bytes = match axum::body::to_bytes(body, 32 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    if bytes.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    *state.last_jpeg.write() = bytes.to_vec();
    StatusCode::NO_CONTENT.into_response()
}

async fn get_last(State(state): State<Arc<IngestShared>>) -> impl IntoResponse {
    let g = state.last_jpeg.read();
    if g.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(g.clone()))
        .unwrap()
        .into_response()
}

async fn get_health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

fn build_router(state: Arc<IngestShared>) -> Router {
    Router::new()
        .route("/ingest", post(post_ingest))
        .route("/last.jpg", get(get_last))
        .route("/health", get(get_health))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

fn bind_addr() -> SocketAddr {
    let s = std::env::var("PIRATE_DISPLAY_INGEST_BIND").unwrap_or_else(|_| "0.0.0.0:0".to_string());
    s.parse().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
}

/// Starts `POST /ingest` + `GET /last.jpg` once per process. Subsequent calls update optional Bearer token and return the same port.
pub fn spawn_display_ingest_server(token: Option<String>) -> io::Result<u16> {
    let _lock = DISPLAY_INGEST_SPAWN_LOCK.lock();

    if let Some(ref s) = *DISPLAY_INGEST_SHARED.lock() {
        *s.token.write() = token.clone();
        let p = DISPLAY_INGEST_PORT.load(Ordering::SeqCst);
        if p != 0 {
            return Ok(p);
        }
    }

    if DISPLAY_INGEST_STARTED.load(Ordering::SeqCst) {
        return Ok(DISPLAY_INGEST_PORT.load(Ordering::SeqCst));
    }

    let shared = Arc::new(IngestShared::new(token));
    *DISPLAY_INGEST_SHARED.lock() = Some(shared.clone());

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(%e, "display ingest runtime");
                DISPLAY_INGEST_STARTED.store(false, Ordering::SeqCst);
                *DISPLAY_INGEST_SHARED.lock() = None;
                return;
            }
        };
        rt.block_on(async move {
            let addr = bind_addr();
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(%e, "display ingest bind");
                    DISPLAY_INGEST_STARTED.store(false, Ordering::SeqCst);
                    *DISPLAY_INGEST_SHARED.lock() = None;
                    return;
                }
            };
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            DISPLAY_INGEST_PORT.store(port, Ordering::SeqCst);
            DISPLAY_INGEST_STARTED.store(true, Ordering::SeqCst);
            if tx.send(port).is_err() {
                return;
            }
            let app = build_router(shared);
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(%e, "display ingest server");
            }
        });
    });

    rx.recv()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "display ingest channel closed"))
}

/// `http://127.0.0.1:PORT` when ingest has been started.
pub fn display_ingest_api_base() -> Option<String> {
    let p = DISPLAY_INGEST_PORT.load(Ordering::SeqCst);
    if p == 0 {
        None
    } else {
        Some(format!("http://127.0.0.1:{p}"))
    }
}

/// Full ingest URL path for producers (use LAN IP as host when remote).
pub fn display_ingest_url() -> Option<String> {
    display_ingest_api_base().map(|b| format!("{b}/ingest"))
}
