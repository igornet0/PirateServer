//! REST for managed gRPC proxy sessions (inbounds): policy with `-1` = unlimited in JSON.

use crate::error::ApiError;
use crate::proxy_tunnel_redis;
use crate::{check_api_bearer, project_or_default, ApiState};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use deploy_proto::deploy::{
    ProxyAccessSchedule, ProxyAccessWindow, ProxyConnectionPolicy,
};
use serde::{Deserialize, Serialize};
use xray_export::{build_xray_client_config, XrayExportOptions};

#[derive(Deserialize)]
pub struct ProxySessionsQuery {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub revoked: Option<bool>,
}

#[derive(Deserialize)]
pub struct ProxyPolicyIn {
    pub max_session_duration_sec: Option<i64>,
    pub traffic_total_bytes: Option<i64>,
    pub traffic_bytes_in_limit: Option<i64>,
    pub traffic_bytes_out_limit: Option<i64>,
    pub active_idle_timeout_sec: Option<u32>,
    pub access_schedule: Option<serde_json::Value>,
    #[serde(default)]
    pub never_expires: Option<bool>,
    #[serde(default)]
    pub limit_duration_by_active_time: Option<bool>,
    /// Omitted = server default (`DEPLOY_DEFAULT_MAX_DEVICES_PER_SESSION`); `-1` = unlimited; `>= 0` = explicit cap.
    #[serde(default)]
    pub max_concurrent_devices_per_session: Option<i64>,
}

#[derive(Deserialize)]
pub struct IngressIn {
    pub protocol: i32,
    pub listen_port: u32,
    #[serde(default)]
    pub listen_udp_port: Option<u32>,
    pub config: serde_json::Value,
    #[serde(default)]
    pub tls: Option<serde_json::Value>,
    #[serde(default)]
    pub template_version: Option<u32>,
}

#[derive(Deserialize)]
pub struct CreateProxySessionBody {
    pub board_label: String,
    pub policy: ProxyPolicyIn,
    #[serde(default)]
    pub recipient_client_pubkey_b64: Option<String>,
    pub wire_mode: Option<i32>,
    /// Object or string; stringified for deploy-server `wire_config_json`.
    #[serde(default)]
    pub wire_config: Option<serde_json::Value>,
    #[serde(default)]
    pub ingress: Option<IngressIn>,
}

#[derive(Deserialize)]
pub struct PatchProxySessionBody {
    pub policy: ProxyPolicyIn,
    #[serde(default)]
    pub wire_mode: Option<i32>,
    #[serde(default)]
    pub wire_config: Option<serde_json::Value>,
    #[serde(default)]
    pub ingress: Option<IngressIn>,
    /// When true, clears public ingress for this session (`ingress_protocol` = 0 on server).
    #[serde(default)]
    pub ingress_clear: Option<bool>,
}

#[derive(Serialize)]
pub struct ProxyPolicyOut {
    pub max_session_duration_sec: i64,
    pub traffic_total_bytes: i64,
    pub traffic_bytes_in_limit: i64,
    pub traffic_bytes_out_limit: i64,
    pub active_idle_timeout_sec: Option<u32>,
    pub access_schedule: Option<serde_json::Value>,
    pub never_expires: bool,
    pub limit_duration_by_active_time: bool,
    /// `None` = server default; `Some(-1)` = unlimited; `Some(n)` = explicit cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_devices_per_session: Option<i64>,
}

#[derive(Serialize)]
pub struct ProxySessionRowOut {
    pub session_id: String,
    pub board_label: String,
    pub client_pubkey_b64: String,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub policy: ProxyPolicyOut,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub active_ms: i64,
    pub last_activity_unix_ms: i64,
    pub revoked: bool,
    pub wire_mode: Option<i32>,
    pub wire_config: Option<serde_json::Value>,
    pub ingress_protocol: Option<i32>,
    pub ingress_listen_port: Option<i32>,
    pub ingress_listen_udp_port: Option<i32>,
    pub ingress_config: Option<serde_json::Value>,
    pub ingress_tls: Option<serde_json::Value>,
    pub ingress_template_version: i32,
    /// Open proxy tunnels for this session (Redis `DEPLOY_REDIS_URL`); `None` if Redis not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_tunnels_online: Option<u64>,
    /// Cumulative tunnel opens tracked in Redis since keys exist; `None` if Redis not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_tunnels_total: Option<u64>,
}

#[derive(Serialize)]
pub struct ProxySessionsPage {
    pub items: Vec<ProxySessionRowOut>,
}

#[derive(Serialize)]
pub struct CreateProxySessionResponse {
    pub session_id: String,
    pub session_token: String,
    pub expires_at_unix_ms: i64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_url: Option<String>,
    /// Public URL for `GET` JSON (gRPC endpoint + session metadata); needs `DEPLOY_SUBSCRIPTION_PUBLIC_HOST` and `DEPLOY_GRPC_PUBLIC_URL`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pirate_bootstrap_url: Option<String>,
}

#[derive(Serialize)]
pub struct PatchProxySessionResponse {
    pub status: String,
    pub expires_at_unix_ms: i64,
}

#[derive(Serialize)]
pub struct RevokeProxySessionResponse {
    pub revoked: u64,
}

#[derive(Deserialize)]
struct AccessScheduleJson {
    timezone: String,
    windows: Vec<AccessWindowJson>,
}

#[derive(Deserialize)]
struct AccessWindowJson {
    days: Vec<u32>,
    start: String,
    end: String,
}

#[derive(Deserialize)]
struct StoredPolicy {
    max_session_duration_sec: Option<u64>,
    traffic_total_bytes: Option<u64>,
    traffic_bytes_in_limit: Option<u64>,
    traffic_bytes_out_limit: Option<u64>,
    active_idle_timeout_sec: Option<u32>,
    access_schedule: Option<serde_json::Value>,
    #[serde(default)]
    never_expires: bool,
    #[serde(default)]
    limit_duration_by_active_time: bool,
    #[serde(default)]
    max_concurrent_devices_per_session: Option<u32>,
}

fn traffic_to_json(lim: Option<u64>) -> i64 {
    match lim {
        None | Some(0) => -1,
        Some(n) => n as i64,
    }
}

fn duration_to_json(sec: Option<u64>) -> i64 {
    match sec {
        None | Some(0) => -1,
        Some(n) => n as i64,
    }
}

fn row_to_out(row: deploy_db::GrpcProxySessionRow) -> Result<ProxySessionRowOut, ApiError> {
    let expires_at_ms = row.expires_at.timestamp_millis();
    let p: StoredPolicy = serde_json::from_str(&row.policy_json).unwrap_or(StoredPolicy {
        max_session_duration_sec: None,
        traffic_total_bytes: None,
        traffic_bytes_in_limit: None,
        traffic_bytes_out_limit: None,
        active_idle_timeout_sec: None,
        access_schedule: None,
        never_expires: false,
        limit_duration_by_active_time: false,
        max_concurrent_devices_per_session: None,
    });
    let expires_display = if p.never_expires || p.max_session_duration_sec.is_none() {
        -1_i64
    } else {
        expires_at_ms
    };
    let policy = ProxyPolicyOut {
        max_session_duration_sec: duration_to_json(p.max_session_duration_sec),
        traffic_total_bytes: traffic_to_json(p.traffic_total_bytes),
        traffic_bytes_in_limit: traffic_to_json(p.traffic_bytes_in_limit),
        traffic_bytes_out_limit: traffic_to_json(p.traffic_bytes_out_limit),
        active_idle_timeout_sec: p.active_idle_timeout_sec,
        access_schedule: p.access_schedule,
        never_expires: p.never_expires,
        limit_duration_by_active_time: p.limit_duration_by_active_time,
        max_concurrent_devices_per_session: match p.max_concurrent_devices_per_session {
            None => None,
            Some(0) => Some(-1),
            Some(n) => Some(n as i64),
        },
    };
    let wire_config = match row.wire_config_json.as_deref() {
        None => None,
        Some(s) => mask_wire_config_json(s)?,
    };
    let ingress_config = match row.ingress_config_json.as_deref() {
        None => None,
        Some(s) => mask_wire_config_json(s)?,
    };
    let ingress_tls = match row.ingress_tls_json.as_deref() {
        None => None,
        Some(s) => mask_tls_json(s)?,
    };
    Ok(ProxySessionRowOut {
        session_id: row.session_id,
        board_label: row.board_label,
        client_pubkey_b64: row.client_pubkey_b64,
        created_at_unix_ms: row.created_at.timestamp_millis(),
        expires_at_unix_ms: expires_display,
        policy,
        bytes_in: row.bytes_in,
        bytes_out: row.bytes_out,
        active_ms: row.active_ms,
        last_activity_unix_ms: row
            .last_activity_at
            .map(|t| t.timestamp_millis())
            .unwrap_or(0),
        revoked: row.revoked,
        wire_mode: row.wire_mode.map(|x| x as i32),
        wire_config,
        ingress_protocol: row.ingress_protocol.map(|x| x as i32),
        ingress_listen_port: row.ingress_listen_port,
        ingress_listen_udp_port: row.ingress_listen_udp_port,
        ingress_config,
        ingress_tls,
        ingress_template_version: row.ingress_template_version,
        proxy_tunnels_online: None,
        proxy_tunnels_total: None,
    })
}

fn mask_secrets_in_json(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(o) = v.as_object_mut() {
        for k in ["password", "secret", "psk"] {
            if o.contains_key(k) {
                o.insert(k.to_string(), serde_json::json!("***"));
            }
        }
    }
    v
}

fn mask_wire_config_json(raw: &str) -> Result<Option<serde_json::Value>, ApiError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| ApiError::bad_request(format!("wire_config: {e}")))?;
    Ok(Some(mask_secrets_in_json(v)))
}

fn mask_tls_json(raw: &str) -> Result<Option<serde_json::Value>, ApiError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| ApiError::bad_request(format!("ingress_tls: {e}")))?;
    Ok(Some(mask_secrets_in_json(v)))
}

fn optional_u64_traffic(v: Option<i64>) -> Result<Option<u64>, ApiError> {
    match v {
        None | Some(-1) => Ok(None),
        Some(n) if n >= 0 => Ok(Some(n as u64)),
        _ => Err(ApiError::bad_request(
            "traffic and duration limits must be >= 0 or -1 (unlimited)",
        )),
    }
}

fn optional_u64_duration(v: Option<i64>) -> Result<Option<u64>, ApiError> {
    match v {
        None | Some(-1) => Ok(None),
        Some(n) if n >= 0 => Ok(Some(n as u64)),
        _ => Err(ApiError::bad_request(
            "max_session_duration_sec must be >= 0 or -1",
        )),
    }
}

fn policy_in_to_proto(p: ProxyPolicyIn) -> Result<ProxyConnectionPolicy, ApiError> {
    let mut out = ProxyConnectionPolicy::default();
    out.max_session_duration_sec = optional_u64_duration(p.max_session_duration_sec)?;
    out.traffic_total_bytes = optional_u64_traffic(p.traffic_total_bytes)?;
    out.traffic_bytes_in_limit = optional_u64_traffic(p.traffic_bytes_in_limit)?;
    out.traffic_bytes_out_limit = optional_u64_traffic(p.traffic_bytes_out_limit)?;
    out.active_idle_timeout_sec = p.active_idle_timeout_sec;
    if let Some(s) = p.access_schedule {
        let sj: AccessScheduleJson =
            serde_json::from_value(s).map_err(|e| ApiError::bad_request(format!("access_schedule: {e}")))?;
        out.access_schedule = Some(ProxyAccessSchedule {
            timezone: sj.timezone,
            windows: sj
                .windows
                .into_iter()
                .map(|w| ProxyAccessWindow {
                    days: w.days,
                    start: w.start,
                    end: w.end,
                })
                .collect(),
        });
    }
    if let Some(b) = p.never_expires {
        out.never_expires = Some(b);
    }
    if let Some(b) = p.limit_duration_by_active_time {
        out.limit_duration_by_active_time = Some(b);
    }
    if let Some(n) = p.max_concurrent_devices_per_session {
        if n == -1 {
            out.max_concurrent_devices_per_session = Some(0);
        } else if n >= 0 {
            out.max_concurrent_devices_per_session = Some(n as u32);
        } else {
            return Err(ApiError::bad_request(
                "max_concurrent_devices_per_session must be >= 0 or -1 (unlimited)",
            ));
        }
    }
    Ok(out)
}

fn wire_config_to_string(v: Option<serde_json::Value>) -> Result<Option<String>, ApiError> {
    match v {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => {
            if s.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(s))
            }
        }
        Some(o) => serde_json::to_string(&o)
            .map(Some)
            .map_err(|e| ApiError::bad_request(e.to_string())),
    }
}

pub async fn api_proxy_sessions_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProxySessionsQuery>,
) -> Result<Json<ProxySessionsPage>, ApiError> {
    check_api_bearer(&state, &headers)?;
    let _pid = project_or_default(&q.project);
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = state
        .plane
        .list_proxy_invitations(limit, offset, q.revoked)
        .await?;
    let session_ids: Vec<String> = rows.iter().map(|r| r.session_id.clone()).collect();
    let redis_stats = match &state.tunnel_redis {
        Some(client) => proxy_tunnel_redis::fetch_session_tunnel_stats(client, &session_ids)
            .await
            .ok(),
        None => None,
    };
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut out = row_to_out(row)?;
        if let Some(ref m) = redis_stats {
            if let Some(&(online, total)) = m.get(&out.session_id) {
                out.proxy_tunnels_online = Some(online);
                out.proxy_tunnels_total = Some(total);
            }
        }
        items.push(out);
    }
    Ok(Json(ProxySessionsPage { items }))
}

pub async fn api_proxy_sessions_create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ProxySessionsQuery>,
    Json(body): Json<CreateProxySessionBody>,
) -> Result<Json<CreateProxySessionResponse>, ApiError> {
    check_api_bearer(&state, &headers)?;
    let project_id = project_or_default(&q.project);
    let policy = policy_in_to_proto(body.policy)?;
    let wire_json = wire_config_to_string(body.wire_config)?;
    let (
        ing_proto,
        ing_lp,
        ing_lup,
        ing_cfg,
        ing_tls,
        ing_ver,
    ) = match body.ingress {
        None => (None, None, None, None, None, None),
        Some(ref i) => {
            let cfg = serde_json::to_string(&i.config)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let tls = match &i.tls {
                None => None,
                Some(t) => Some(serde_json::to_string(t).map_err(|e| ApiError::bad_request(e.to_string()))?),
            };
            (
                Some(i.protocol),
                Some(i.listen_port),
                i.listen_udp_port,
                Some(cfg),
                tls,
                i.template_version,
            )
        }
    };
    let r = state
        .plane
        .create_proxy_invitation(
            project_id,
            body.board_label,
            policy,
            body.recipient_client_pubkey_b64,
            body.wire_mode,
            wire_json,
            ing_proto,
            ing_lp,
            ing_lup,
            ing_cfg,
            ing_tls,
            ing_ver,
        )
        .await?;
    let subscription_url = r.subscription_url.clone().or_else(|| {
        let tok = r.subscription_token.as_ref()?;
        let base = state.subscription_public_base.as_deref()?.trim().trim_end_matches('/');
        if base.is_empty() {
            return None;
        }
        Some(format!("{base}/api/v1/public/proxy-subscription/{tok}"))
    });
    let pirate_bootstrap_url = r.subscription_token.as_ref().and_then(|tok| {
        let base = state.subscription_public_base.as_deref()?.trim().trim_end_matches('/');
        if base.is_empty() {
            return None;
        }
        if state.grpc_public_url.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none() {
            return None;
        }
        Some(format!("{base}/api/v1/public/pirate-bootstrap/{tok}"))
    });
    Ok(Json(CreateProxySessionResponse {
        session_id: r.session_id,
        session_token: r.session_token,
        expires_at_unix_ms: r.expires_at_unix_ms,
        status: r.status,
        subscription_token: r.subscription_token,
        subscription_url,
        pirate_bootstrap_url,
    }))
}

pub async fn api_proxy_sessions_patch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(q): Query<ProxySessionsQuery>,
    Json(body): Json<PatchProxySessionBody>,
) -> Result<Json<PatchProxySessionResponse>, ApiError> {
    check_api_bearer(&state, &headers)?;
    let project_id = project_or_default(&q.project);
    let policy = policy_in_to_proto(body.policy)?;
    let wire_json = wire_config_to_string(body.wire_config)?;
    let (
        ing_proto,
        ing_lp,
        ing_lup,
        ing_cfg,
        ing_tls,
        ing_ver,
    ) = if body.ingress_clear == Some(true) {
        (Some(0), None, None, None, None, None)
    } else {
        match body.ingress {
            None => (None, None, None, None, None, None),
            Some(ref i) => {
                let cfg = serde_json::to_string(&i.config)
                    .map_err(|e| ApiError::bad_request(e.to_string()))?;
                let tls = match &i.tls {
                    None => None,
                    Some(t) => Some(serde_json::to_string(t).map_err(|e| ApiError::bad_request(e.to_string()))?),
                };
                (
                    Some(i.protocol),
                    Some(i.listen_port),
                    i.listen_udp_port,
                    Some(cfg),
                    tls,
                    i.template_version,
                )
            }
        }
    };
    let r = state
        .plane
        .update_proxy_invitation(
            project_id,
            session_id,
            policy,
            body.wire_mode,
            wire_json,
            ing_proto,
            ing_lp,
            ing_lup,
            ing_cfg,
            ing_tls,
            ing_ver,
        )
        .await?;
    Ok(Json(PatchProxySessionResponse {
        status: r.status,
        expires_at_unix_ms: r.expires_at_unix_ms,
    }))
}

pub async fn api_proxy_sessions_revoke(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<RevokeProxySessionResponse>, ApiError> {
    check_api_bearer(&state, &headers)?;
    let n = state
        .plane
        .revoke_proxy_invitation(&session_id)
        .await?;
    Ok(Json(RevokeProxySessionResponse { revoked: n }))
}

fn xray_doc_for_row(
    state: &ApiState,
    row: &deploy_db::GrpcProxySessionRow,
) -> Result<serde_json::Value, ApiError> {
    let host = state.subscription_server_hostname.as_deref().ok_or_else(|| {
        ApiError::service_unavailable(
            "set DEPLOY_SUBSCRIPTION_PUBLIC_HOST (or CONTROL_API_SUBSCRIPTION_PUBLIC_HOST) to the public hostname used for ingress",
        )
    })?;
    if host.trim().is_empty() {
        return Err(ApiError::service_unavailable(
            "DEPLOY_SUBSCRIPTION_PUBLIC_HOST is empty",
        ));
    }
    build_xray_client_config(
        row,
        &XrayExportOptions {
            server_address: host.trim().to_string(),
            tls_server_name: state.subscription_tls_sni.clone(),
        },
    )
    .map_err(ApiError::bad_request)
}

pub async fn api_public_proxy_subscription(
    State(state): State<ApiState>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = state
        .plane
        .fetch_proxy_invitation_by_subscription_token(token.trim())
        .await?
        .ok_or_else(|| ApiError::not_found("unknown subscription"))?;
    if row.revoked {
        return Err(ApiError::not_found("subscription revoked"));
    }
    if chrono::Utc::now() > row.expires_at {
        return Err(ApiError::not_found("subscription expired"));
    }
    if row.ingress_protocol.is_none() {
        return Err(ApiError::not_found(
            "public ingress is not enabled for this session",
        ));
    }
    let doc = xray_doc_for_row(&state, &row)?;
    Ok(Json(doc))
}

/// Public JSON for pirate-client: gRPC URL (from env) plus session metadata. Session auth token is not stored server-side; use `session_token` from create response or a full Inbounds export.
#[derive(Serialize)]
pub struct PirateBootstrapOut {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub version: u32,
    pub grpc_url: String,
    pub session_id: String,
    pub board_label: String,
    pub subscription_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token_note: Option<&'static str>,
    pub expires_at_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_mode: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
}

pub async fn api_public_pirate_bootstrap(
    State(state): State<ApiState>,
    Path(token): Path<String>,
) -> Result<Json<PirateBootstrapOut>, ApiError> {
    let grpc_url = state.grpc_public_url.as_deref().ok_or_else(|| {
        ApiError::service_unavailable(
            "set DEPLOY_GRPC_PUBLIC_URL (or CONTROL_API_GRPC_PUBLIC_URL) so clients know the gRPC endpoint",
        )
    })?;
    if grpc_url.trim().is_empty() {
        return Err(ApiError::service_unavailable(
            "DEPLOY_GRPC_PUBLIC_URL is empty",
        ));
    }
    let row = state
        .plane
        .fetch_proxy_invitation_by_subscription_token(token.trim())
        .await?
        .ok_or_else(|| ApiError::not_found("unknown subscription"))?;
    if row.revoked {
        return Err(ApiError::not_found("subscription revoked"));
    }
    if chrono::Utc::now() > row.expires_at {
        return Err(ApiError::not_found("subscription expired"));
    }
    let wire_config = row
        .wire_config_json
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str(s).ok());
    let policy = serde_json::from_str(&row.policy_json).ok();
    let subscription_token = row
        .subscription_token
        .clone()
        .unwrap_or_else(|| token.trim().to_string());
    Ok(Json(PirateBootstrapOut {
        kind: "pirate-bootstrap",
        version: 1,
        grpc_url: grpc_url.to_string(),
        session_id: row.session_id,
        board_label: row.board_label,
        subscription_token,
        session_token: None,
        session_token_note: Some(
            "Session auth secret was shown only once at creation; paste it from that dialog or from a full Inbounds export (Copy JSON).",
        ),
        expires_at_unix_ms: row.expires_at.timestamp_millis(),
        wire_mode: row.wire_mode.map(|x| x as i32),
        wire_config,
        policy,
    }))
}

pub async fn api_proxy_session_xray_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_api_bearer(&state, &headers)?;
    let row = state
        .plane
        .fetch_proxy_invitation(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("session not found"))?;
    if row.revoked {
        return Err(ApiError::not_found("session revoked"));
    }
    if row.ingress_protocol.is_none() {
        return Err(ApiError::not_found(
            "public ingress is not enabled for this session",
        ));
    }
    let doc = xray_doc_for_row(&state, &row)?;
    Ok(Json(doc))
}
