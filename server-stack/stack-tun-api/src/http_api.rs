//! REST control-plane for profiles, stats, peers.

use crate::request_bus::{execute_bus_request, filtered_journal_snapshot};
use crate::state::SharedState;
use crate::task_queue::{phase_from_query, TaskOuterJson};
use crate::types::{AuditEntry, BusRouteDecisionKind, PersistRoot, StackTunRouteRule, TunnelProfile};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose::STANDARD as B64_STANDARD, Engine as _};
use deploy_proto::stack_tun::{BusKv, BusRequestEnvelope};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct HttpState {
    pub inner: Arc<SharedState>,
}

fn check_rest_auth(state: &HttpState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let Some(expect) = &state.inner.rest_bearer_token else {
        return Ok(());
    };
    if expect.trim().is_empty() {
        return Ok(());
    }
    let got = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let wanted = format!("Bearer {expect}");
    if got.trim() != wanted.trim() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

fn audit(state: &HttpState, level: &str, msg: impl Into<String>) {
    let mut g = state.inner.audit.lock();
    if g.len() >= 512 {
        g.pop_front();
    }
    g.push_back(AuditEntry {
        ts_unix_ms: deploy_auth::now_unix_ms(),
        level: level.to_string(),
        message: msg.into(),
    });
}

async fn health() -> &'static str {
    "ok"
}

async fn get_config(State(state): State<HttpState>, headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let root = state.inner.store.read_root().await;
    serde_json::to_value(root).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutProfilesBody {
    profiles: Vec<TunnelProfile>,
}

async fn put_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(body): Json<PutProfilesBody>,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let mut seen = std::collections::HashSet::<String>::new();
    for p in &body.profiles {
        if seen.contains(&p.id) {
            audit(&state, "error", format!("duplicate profile id {}", p.id));
            return Err(StatusCode::BAD_REQUEST);
        }
        seen.insert(p.id.clone());
        if let Err(e) = p.validate() {
            audit(&state, "error", format!("profile {} invalid: {e}", p.id));
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    let prev = state.inner.store.read_root().await;
    let routes = prev.routes.clone();
    let root = PersistRoot {
        version: 1,
        profiles: body.profiles,
        routes,
    };
    state.inner.store.replace_root(root).await;
    if let Err(e) = state.inner.store.save_to_disk().await {
        tracing::error!("save profiles: {e}");
        audit(&state, "error", format!("save profiles: {e}"));
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    state.inner.resync_all().await;
    audit(&state, "info", "profiles updated via REST");
    Ok(Json(json!({"status":"ok"})))
}

async fn post_reload_peers(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    state.inner.reload_peers_disk().await.map_err(|e| {
        tracing::warn!("reload peers: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    audit(&state, "info", "authorized_peers.json reloaded");
    Ok(Json(json!({"status":"ok"})))
}

async fn get_identity_public_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let public_key_b64 = deploy_auth::pubkey_b64_url(&state.inner.signing_key);
    Ok(Json(json!({ "publicKeyB64": public_key_b64 })))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutAuthorizedPeerBody {
    public_key_b64: String,
}

async fn post_authorized_peer(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(body): Json<PutAuthorizedPeerBody>,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let vk = deploy_auth::parse_verifying_key_b64(&body.public_key_b64).map_err(|e| {
        tracing::warn!("invalid stack-tun peer public key: {e}");
        StatusCode::BAD_REQUEST
    })?;
    let mut peers = deploy_auth::load_authorized_peers(&state.inner.peers_path).map_err(|e| {
        tracing::warn!("load authorized peers: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let inserted = peers.insert(*vk.as_bytes());
    deploy_auth::save_authorized_peers(&state.inner.peers_path, &peers).map_err(|e| {
        tracing::warn!("save authorized peers: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    state.inner.reload_peers_disk().await.map_err(|e| {
        tracing::warn!("reload authorized peers after upsert: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    audit(
        &state,
        "info",
        format!(
            "authorized peer {} {}",
            if inserted { "added" } else { "already-present" },
            body.public_key_b64
        ),
    );
    Ok(Json(json!({
        "status": "ok",
        "inserted": inserted,
        "publicKeyB64": body.public_key_b64,
    })))
}

async fn get_peers(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let peers = state.inner.peers.read().await;
    let list: Vec<String> = peers.iter().map(deploy_auth::raw_pubkey_b64_url).collect();
    Ok(Json(json!({ "peers": list })))
}

async fn get_routes(State(state): State<HttpState>, headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let root = state.inner.store.read_root().await;
    Ok(Json(json!({ "routes": root.routes })))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutRoutesBody {
    routes: Vec<StackTunRouteRule>,
}

async fn put_routes(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(body): Json<PutRoutesBody>,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let mut root = state.inner.store.read_root().await;
    root.routes = body.routes;
    state.inner.store.replace_root(root.clone()).await;
    if let Err(e) = state.inner.store.save_to_disk().await {
        tracing::error!("save routes/profiles root: {e}");
        audit(&state, "error", format!("save routes: {e}"));
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    audit(&state, "info", "routes updated via REST");
    Ok(Json(json!({ "status": "ok" })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestLogQuery {
    #[serde(default = "default_journal_limit")]
    limit: usize,
    source: Option<String>,
    target: Option<String>,
    profile_id: Option<String>,
    status: Option<u32>,
    host: Option<String>,
    path: Option<String>,
    method: Option<String>,
    trace_id: Option<String>,
    request_id: Option<String>,
    #[serde(default)]
    errors_only: Option<String>,
    #[serde(default)]
    blocked_only: Option<String>,
}

fn qs_truthy(v: Option<&String>) -> bool {
    matches!(
        v.map(|s| s.trim()),
        Some(s) if s.eq_ignore_ascii_case("true") || s == "1" || s.eq_ignore_ascii_case("yes")
    )
}

fn default_journal_limit() -> usize {
    80
}

async fn get_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(q): Query<RequestLogQuery>,
) -> Result<Json<Vec<crate::types::RequestBusJournalEntry>>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let rows = filtered_journal_snapshot(
        state.inner.as_ref(),
        q.limit,
        q.source.as_deref(),
        q.target.as_deref(),
        q.profile_id.as_deref(),
        q.status,
        q.host.as_deref(),
        q.path.as_deref(),
        q.method.as_deref(),
        q.trace_id.as_deref(),
        q.request_id.as_deref(),
        qs_truthy(q.errors_only.as_ref()),
        qs_truthy(q.blocked_only.as_ref()),
    );
    Ok(Json(rows))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BusInvokeBody {
    profile_id: String,
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    trace_id: String,
    #[serde(default)]
    source_node_id: String,
    #[serde(default)]
    target_node_id: String,
    #[serde(default)]
    hop_id: u32,
    method: String,
    #[serde(default)]
    scheme: String,
    host: String,
    path: String,
    #[serde(default)]
    headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    body_base64: String,
}

async fn post_request_bus_invoke(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(body): Json<BusInvokeBody>,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let body_bytes = if body.body_base64.trim().is_empty() {
        vec![]
    } else {
        B64_STANDARD
            .decode(body.body_base64.trim())
            .map_err(|_| StatusCode::BAD_REQUEST)?
    };

    state
        .inner
        .stats
        .request_bus_received
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let rules = {
        let root = state.inner.store.read_root().await;
        root.routes.clone()
    };

    let exec = execute_bus_request(
        &state.inner,
        &rules,
        body.profile_id.trim(),
        if body.request_id.trim().is_empty() {
            format!("http-{}", deploy_auth::now_unix_ms())
        } else {
            body.request_id.clone()
        },
        body.trace_id.clone(),
        body.source_node_id.clone(),
        body.target_node_id.clone(),
        body.hop_id,
        body.method.clone(),
        body.scheme.clone(),
        body.host.clone(),
        body.path.clone(),
        body.headers.clone(),
        body_bytes,
    )
    .await;

    match exec {
        Ok((status, hm, bytes, decision)) => {
            if matches!(decision, BusRouteDecisionKind::Deny) {
                state
                    .inner
                    .stats
                    .request_bus_blocked
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            if status >= 400 {
                state
                    .inner
                    .stats
                    .request_bus_errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            state
                .inner
                .stats
                .request_bus_completed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let mut hdr_obj = serde_json::Map::new();
            for (name, v) in hm.iter() {
                if let Ok(s) = v.to_str() {
                    hdr_obj.insert(name.as_str().to_string(), json!(s));
                }
            }

            Ok(Json(json!({
                "status": status,
                "decision": format!("{:?}", decision).to_lowercase(),
                "headers": hdr_obj,
                "bodyBase64": B64_STANDARD.encode(bytes),
            })))
        }
        Err(e) => {
            state
                .inner
                .stats
                .request_bus_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(Json(json!({
                "status": 502,
                "error": e,
            })))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskListQs {
    profile_id: String,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskGetQs {
    profile_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskSubmitQs {
    /// Long-poll up to `waitMs` for terminal phase (`completed` / `failed` / `expired`).
    #[serde(default)]
    wait_ms: Option<u64>,
}

fn invoke_body_decode(body_base64_trim: &str) -> Result<Vec<u8>, StatusCode> {
    if body_base64_trim.is_empty() {
        Ok(vec![])
    } else {
        B64_STANDARD
            .decode(body_base64_trim)
            .map_err(|_| StatusCode::BAD_REQUEST)
    }
}

async fn poll_task_snap_json(
    queue: std::sync::Arc<crate::task_queue::TaskQueue>,
    request_id: &str,
    wait: std::time::Duration,
) -> Result<TaskOuterJson, StatusCode> {
    let deadline = std::time::Instant::now() + wait.max(std::time::Duration::from_millis(250));
    loop {
        let snap = queue
            .snapshot_json(request_id)
            .ok_or(StatusCode::NOT_FOUND)?;
        if matches!(
            snap.phase_str.as_str(),
            "completed" | "failed" | "expired"
        ) {
            return Ok(snap);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(snap);
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn post_task_submit(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(q): Query<TaskSubmitQs>,
    Json(body): Json<BusInvokeBody>,
) -> Result<(axum::http::StatusCode, Json<Value>), StatusCode> {
    check_rest_auth(&state, &headers)?;
    let body_bytes =
        invoke_body_decode(body.body_base64.trim())?;

    let pid = body.profile_id.trim();
    if pid.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let rq = if body.request_id.trim().is_empty() {
        String::new()
    } else {
        body.request_id.clone()
    };

    let headers_kv: Vec<BusKv> = body
        .headers
        .iter()
        .map(|(k, v)| BusKv {
            key: k.clone(),
            value: v.clone(),
        })
        .collect();

    let env = BusRequestEnvelope {
        request_id: rq,
        trace_id: body.trace_id.clone(),
        hop_id: body.hop_id,
        source_node_id: body.source_node_id.clone(),
        target_node_id: body.target_node_id.clone(),
        method: body.method.clone(),
        scheme: body.scheme.clone(),
        host: body.host.clone(),
        path: body.path.clone(),
        headers: headers_kv,
        body_chunk: body_bytes,
        body_finished: true,
        created_at_ms: deploy_auth::now_unix_ms(),
    };

    let tq = {
        let mq = state.inner.task_queues.lock();
        mq.get(pid).cloned()
    }
    .ok_or(StatusCode::NOT_FOUND)?;

    let rid =
        Arc::clone(&tq)
            .enqueue(&state.inner.stats, env)
            .map_err(|e| match e {
                "task_queue_full" => StatusCode::SERVICE_UNAVAILABLE,
                "duplicate_request_id" => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            })?;

    if let Some(w) = q.wait_ms.filter(|&x| x > 0) {
        let waited = poll_task_snap_json(
            Arc::clone(&tq),
            &rid,
            std::time::Duration::from_millis(w.min(120_000)),
        )
        .await?;
        return Ok((
            axum::http::StatusCode::OK,
            Json(serde_json::to_value(waited).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?),
        ));
    }

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(json!({"requestId": rid, "phase": "queued", "profileId": pid })),
    ))
}

async fn list_tasks(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(q): Query<TaskListQs>,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let pid = q.profile_id.trim();
    if pid.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let lim = q.limit.unwrap_or(100).min(500).max(1);
    let ph = phase_from_query(q.phase.as_deref());
    let tq = {
        let mq = state.inner.task_queues.lock();
        mq.get(pid).cloned()
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    let rows = tq.snapshot_list_json(ph, lim);
    let v = serde_json::to_value(rows).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(v))
}

async fn get_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Query(q): Query<TaskGetQs>,
) -> Result<Json<Value>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let pid = q.profile_id.trim();
    if pid.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let tq = {
        let mq = state.inner.task_queues.lock();
        mq.get(pid).cloned()
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    let row = tq
        .snapshot_json(&request_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let v = serde_json::to_value(row).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(v))
}

async fn get_stats(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<crate::types::TunnelStatsSnapshot>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    Ok(Json(state.inner.stats.snapshot()))
}

async fn get_audit(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditEntry>>, StatusCode> {
    check_rest_auth(&state, &headers)?;
    let g = state.inner.audit.lock();
    Ok(Json(g.iter().cloned().collect()))
}

pub fn router(st: Arc<SharedState>) -> Router {
    let state = HttpState { inner: st };
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/config", get(get_config).put(put_config))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/audit", get(get_audit))
        .route("/api/v1/identity/public-key", get(get_identity_public_key))
        .route(
            "/api/v1/peers",
            get(get_peers).post(post_authorized_peer),
        )
        .route("/api/v1/peers/reload", post(post_reload_peers))
        .route("/api/v1/routes", get(get_routes).put(put_routes))
        .route("/api/v1/requests", get(get_requests))
        .route("/api/v1/request-bus/invoke", post(post_request_bus_invoke))
        .route("/api/v1/tasks", post(post_task_submit).get(list_tasks))
        .route("/api/v1/tasks/:request_id", get(get_task))
        .with_state(state)
}
