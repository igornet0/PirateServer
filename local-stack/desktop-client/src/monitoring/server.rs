//! Localhost-only HTTP + WebSocket server for monitoring API.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Json;
use axum::Router;
use futures_util::StreamExt;
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use tower_http::cors::{Any, CorsLayer};

use super::alerts::AlertConfig;
use super::collector::{
    collect_cpu_detail, collect_disk_detail, collect_logs_detail, collect_memory_detail,
    collect_network_detail, collect_overview, collect_processes_list, NetCounters,
};
use super::history::HistoryBuffer;
use super::types::*;

static MONITORING_PORT: OnceLock<u16> = OnceLock::new();

/// Set after the monitoring server starts; used for Tauri-side toggles (economy mode).
pub static MONITOR_SHARED: OnceLock<Arc<MonitorShared>> = OnceLock::new();

pub fn monitoring_set_economy_mode(enabled: bool) -> bool {
    MONITOR_SHARED
        .get()
        .map(|s| {
            s.alerts
                .economy_mode
                .store(enabled, std::sync::atomic::Ordering::Relaxed);
            true
        })
        .unwrap_or(false)
}

/// Base URL for the monitoring API, e.g. `http://127.0.0.1:PORT`, after `spawn_monitoring_server` succeeded.
pub fn monitoring_api_base() -> Option<String> {
    MONITORING_PORT
        .get()
        .map(|p| format!("http://127.0.0.1:{}", p))
}

pub struct MonitorShared {
    pub overview: RwLock<Option<MonitoringOverview>>,
    net_prev: RwLock<Option<NetCounters>>,
    pub history: Mutex<HistoryBuffer>,
    pub alerts: AlertConfig,
}

impl MonitorShared {
    fn new() -> Self {
        Self {
            overview: RwLock::new(None),
            net_prev: RwLock::new(None),
            history: Mutex::new(HistoryBuffer::new(HistoryBuffer::default_cap())),
            alerts: AlertConfig::default(),
        }
    }
}

#[derive(Deserialize)]
struct SeriesQuery {
    metric: String,
    #[serde(default = "default_range")]
    range: String,
    #[serde(default = "default_step")]
    step: u64,
}

fn default_range() -> String {
    "1h".to_string()
}

fn default_step() -> u64 {
    5000
}

fn parse_range_ms(s: &str) -> i64 {
    match s {
        "15m" => 15 * 60 * 1000,
        "1h" => 60 * 60 * 1000,
        "24h" => 24 * 60 * 60 * 1000,
        "7d" => 7 * 24 * 60 * 60 * 1000,
        _ => 60 * 60 * 1000,
    }
}

async fn get_overview(State(s): State<Arc<MonitorShared>>) -> impl IntoResponse {
    let g = s.overview.read();
    if let Some(ref o) = *g {
        return Json(o.clone()).into_response();
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": "overview not ready yet" })),
    )
        .into_response()
}

async fn get_detail_cpu(State(_s): State<Arc<MonitorShared>>) -> Json<CpuDetail> {
    match tokio::task::spawn_blocking(|| collect_cpu_detail(20)).await {
        Ok(d) => Json(d),
        Err(_) => Json(collect_cpu_detail(20)),
    }
}

async fn get_detail_memory(State(_s): State<Arc<MonitorShared>>) -> Json<MemoryDetail> {
    match tokio::task::spawn_blocking(|| collect_memory_detail(20)).await {
        Ok(d) => Json(d),
        Err(_) => Json(collect_memory_detail(20)),
    }
}

async fn get_detail_disk(State(_s): State<Arc<MonitorShared>>) -> Json<DiskDetail> {
    match tokio::task::spawn_blocking(|| collect_disk_detail(20)).await {
        Ok(d) => Json(d),
        Err(_) => Json(collect_disk_detail(20)),
    }
}

async fn get_detail_network(State(s): State<Arc<MonitorShared>>) -> Json<NetworkDetail> {
    let prev = s.net_prev.read().clone();
    Json(collect_network_detail(prev.as_ref()))
}

#[derive(Deserialize)]
struct ProcQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    200
}

async fn get_detail_processes(Query(q): Query<ProcQuery>) -> Json<ProcessesDetail> {
    Json(collect_processes_list(&q.q, q.limit))
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default)]
    level: Option<String>,
    #[serde(default = "default_log_limit")]
    limit: usize,
}

fn default_log_limit() -> usize {
    200
}

async fn get_detail_logs(Query(q): Query<LogQuery>) -> Json<LogsDetail> {
    let lev = q.level.as_deref();
    Json(collect_logs_detail(lev, q.limit))
}

async fn get_series(
    State(s): State<Arc<MonitorShared>>,
    Query(q): Query<SeriesQuery>,
) -> Json<SeriesResponse> {
    let range_ms = parse_range_ms(&q.range);
    let h = s.history.lock();
    Json(h.series(&q.metric, range_ms, q.step))
}

async fn get_export(State(s): State<Arc<MonitorShared>>) -> Json<ExportSnapshot> {
    let o = s
        .overview
        .read()
        .clone()
        .unwrap_or_else(|| MonitoringOverview {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            disk: DiskOverview { mounts: vec![] },
            memory: MemoryOverview {
                total_bytes: 0,
                used_bytes: 0,
                available_bytes: 0,
                cached_bytes: None,
                buffers_bytes: None,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
            },
            cpu: CpuOverview {
                usage_percent: 0.0,
                loadavg: LoadAvg {
                    m1: 0.0,
                    m5: 0.0,
                    m15: 0.0,
                },
            },
            temperature_c: None,
            process_count: 0,
            network: NetworkOverview { interfaces: vec![] },
            logs: LogsOverview { items: vec![] },
            warnings: vec!["no snapshot yet".to_string()],
            partial: true,
        });
    Json(ExportSnapshot {
        overview: o.clone(),
        exported_ts_ms: chrono::Utc::now().timestamp_millis(),
    })
}

async fn get_alerts(State(s): State<Arc<MonitorShared>>) -> Json<AlertsStatus> {
    let economy = s.alerts.economy_mode.load(std::sync::atomic::Ordering::Relaxed);
    let enabled = s.alerts.enabled.load(std::sync::atomic::Ordering::Relaxed);
    let triggered = s
        .overview
        .read()
        .as_ref()
        .map(|o| s.alerts.evaluate(o))
        .unwrap_or_default();
    Json(AlertsStatus {
        alerts_enabled: enabled,
        economy_mode: economy,
        triggered,
    })
}

async fn ws_handler(ws: WebSocketUpgrade, State(s): State<Arc<MonitorShared>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, s))
}

async fn handle_ws(mut socket: WebSocket, state: Arc<MonitorShared>) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let snap = state.overview.read().clone();
                if let Some(o) = snap {
                    let msg = serde_json::json!({
                        "type": "tick",
                        "channel": "overview",
                        "payload": o,
                    });
                    if socket.send(Message::Text(msg.to_string())).await.is_err() {
                        break;
                    }
                }
            }
            m = socket.next() => {
                match m {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<StreamSubscribe>(&t) {
                            if v.op == "ping" {
                                let _ = socket.send(Message::Pong(vec![])).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn build_router(state: Arc<MonitorShared>) -> Router {
    Router::new()
        .route("/api/v1/monitoring/overview", get(get_overview))
        .route("/api/v1/monitoring/detail/cpu", get(get_detail_cpu))
        .route("/api/v1/monitoring/detail/memory", get(get_detail_memory))
        .route("/api/v1/monitoring/detail/disk", get(get_detail_disk))
        .route("/api/v1/monitoring/detail/network", get(get_detail_network))
        .route("/api/v1/monitoring/detail/processes", get(get_detail_processes))
        .route("/api/v1/monitoring/detail/logs", get(get_detail_logs))
        .route("/api/v1/monitoring/series", get(get_series))
        .route("/api/v1/monitoring/export", get(get_export))
        .route("/api/v1/monitoring/alerts", get(get_alerts))
        .route("/api/v1/monitoring/stream", get(ws_handler))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

async fn tick_loop(shared: Arc<MonitorShared>) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        if shared
            .alerts
            .economy_mode
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            // Skip every other tick in economy mode (~4s effective cadence).
            static SKIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if SKIP.fetch_xor(true, std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
        }
        let prev = shared.net_prev.read().clone();
        let res = tokio::task::spawn_blocking(move || collect_overview(prev.as_ref())).await;
        let Ok((mut overview, net)) = res else {
            continue;
        };
        let triggered = shared.alerts.evaluate(&overview);
        overview.warnings = triggered;
        *shared.net_prev.write() = Some(net);
        *shared.overview.write() = Some(overview.clone());
        shared.history.lock().record_overview(&overview);
        {
            let _ = super::sqlite_store::append_sample(&overview);
        }
    }
}

/// Binds `127.0.0.1:0`, spawns async runtime thread, returns port when listener is ready.
pub fn spawn_monitoring_server() -> std::io::Result<u16> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(%e, "monitoring runtime");
                return;
            }
        };
        rt.block_on(async move {
            let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(%e, "monitoring bind");
                    return;
                }
            };
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            let _ = MONITORING_PORT.set(port);
            if tx.send(port).is_err() {
                return;
            }
            let shared = Arc::new(MonitorShared::new());
            let _ = MONITOR_SHARED.set(shared.clone());
            tokio::spawn(tick_loop(shared.clone()));
            let app = build_router(shared);
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(%e, "monitoring server");
            }
        });
    });
    rx.recv().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "monitoring channel closed")
    })
}
