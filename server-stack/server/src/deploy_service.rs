//! Deploy: stream → temp file, SHA-256, tar unpack, symlink, process control.

use crate::metrics_http::ProxyTunnelMetrics;
use crate::proxy_session;
use crate::tunnel_admission::{self, TunnelAdmission};
use crate::tunnel_flush::{
    flush_managed_tunnel_end, spawn_managed_checkpoint, ManagedTunnelCheckpoint, TunnelFlushCounters,
};
use crate::tunnel_registry::{redis_fields_snapshot, RedisTunnelDropGuard, TunnelRedis};
use deploy_core::{
    normalize_project_id, project_deploy_root, read_current_version_from_symlink,
    read_host_stack_ui_flags, read_server_stack_bundle_version_from_var_lib, refresh_process_state,
    release_dir_for_version, status_current_version_display, validate_project_id,
    validate_version as validate_version_core, AppState,
};
use crate::auth::ServerAuth;
use crate::session_audit::{peer_ip_from_request, register_authenticated_client, SessionAuditHub};
use deploy_auth::{parse_verifying_key_b64, raw_pubkey_b64_url, META_PUBKEY};
use deploy_db::{DbStore, GrpcProxySessionRow};
use deploy_auth::{
    now_unix_ms, signing_payload, verify_rpc_metadata, verify_upload_metadata,
    verify_upload_server_stack_metadata, META_PROJECT, META_STACK_APPLY_SHA256, META_VERSION,
};
use deploy_control::{
    collect_cpu_detail, collect_disk_detail, collect_host_stats, collect_memory_detail,
    collect_network_detail, collect_processes_list,
    CpuDetail, CpuTimes, DiskDetail, DiskIoSummary, HostLogLine, HostMountStats, HostNetInterface,
    HostStatsView, LoadAvg, MemoryDetail, MemoryOverview, NetworkDetail, ProcessCpu, ProcessDisk,
    ProcessMem, ProcessRow, ProcessesDetail, SeriesHint, NetCounters,
};
use std::collections::HashMap;
use deploy_proto::deploy::{
    deploy_service_server::DeployService, host_stats_detail_response::Detail as HostStatsDetailOneof,
    proxy_client_msg, proxy_server_msg,
    CloseConnectionRequest, CloseConnectionResponse, ConnectionProbeChunk, ConnectionProbeResult,
    CpuDetailProto, CpuTimesProto, CreateConnectionRequest, CreateConnectionResponse,
    DeployChunk, DeployResponse, DiskDetailProto, DiskIoSummaryProto,
    GetStatsRequest, GetStatsResponse,
    HostLogLineProto, HostMountStatsProto, HostNetInterfaceProto, HostStatsDetailKind,
    HostStatsDetailRequest, HostStatsDetailResponse, HostStatsRequest, HostStatsResponse, LoadAvgProto,
    MemoryDetailProto, MemoryOverviewProto, NetworkDetailProto, PairRequest, PairResponse,
    ProcessCpuProto, ProcessDiskProto, ProcessMemProto, ProcessRowProto, ProcessesDetailProto,
    ProxyClientMsg, ProxyOpenResult, ProxyServerMsg,
    DisplayTopologyDisplay, ReportDisplayTopologyRequest, ReportDisplayTopologyResponse,
    ReportResourceUsageRequest, ReportResourceUsageResponse,
    RestartProcessRequest, RollbackRequest, RollbackResponse, SeriesHintProto, StackApplyMode,
    StackApplyOptions, ServerStackChunk, ServerStackInfo, ServerStackInfoRequest, ServerStackResponse,
    StatusRequest, StatusResponse,
    StopProcessRequest,
    ListSessionsRequest, ListSessionsResponse, QuerySessionLogsRequest, QuerySessionLogsResponse,
    SessionLogEvent, SessionPeerRow,
    UpdateConnectionProfileRequest, UpdateConnectionProfileResponse,
    UpdateProxySettingsRequest, UpdateProxySettingsResponse,
};
use futures_util::Stream;
use futures_util::StreamExt;
use prost::Message;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::pin::Pin;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use std::time::Duration;

use crate::auth::{sign_pair_response, verify_pair_signature};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex as TokioMutex};
use uuid::Uuid;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use rand::RngCore as _;
use tracing::{error, info, warn};

pub(crate) fn floor_to_utc_hour(dt: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    let ts = dt.timestamp();
    let hour_floor = ts - (ts.rem_euclid(3600));
    chrono::DateTime::from_timestamp(hour_floor, 0)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or(dt)
}

async fn session_peer_row_enriched(
    db: &DbStore,
    pk: String,
    last_seen_ms: i64,
    last_peer_ip: String,
    last_grpc_method: String,
) -> Result<SessionPeerRow, deploy_db::DbError> {
    let kind_db = db
        .fetch_grpc_peer_profile_kind(&pk)
        .await?
        .unwrap_or(0) as i32;
    let snap = db.fetch_grpc_peer_resource_snapshot(&pk).await?;
    let (proxy_in, proxy_out) = db.sum_grpc_proxy_traffic_totals(&pk).await?;
    let (display_topology, display_stream_capable) =
        match db.fetch_peer_display_topology(&pk).await? {
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
                let dt: Vec<DisplayTopologyDisplay> = rows
                    .into_iter()
                    .map(|r| DisplayTopologyDisplay {
                        index: r.index,
                        label: r.label,
                        width: r.width,
                        height: r.height,
                    })
                    .collect();
                (dt, Some(cap))
            }
        };
    Ok(SessionPeerRow {
        client_public_key_b64: pk,
        last_seen_ms,
        last_peer_ip,
        last_grpc_method,
        connection_kind: kind_db,
        last_cpu_percent: snap.as_ref().and_then(|s| s.cpu_percent),
        last_ram_percent: snap.as_ref().and_then(|s| s.ram_percent),
        last_gpu_percent: snap.as_ref().and_then(|s| s.gpu_percent),
        resource_reported_at_ms: snap
            .as_ref()
            .map(|s| s.reported_at.timestamp_millis())
            .unwrap_or(0),
        ram_used_bytes: snap
            .as_ref()
            .and_then(|s| s.ram_used_bytes)
            .map(|v| v.max(0) as u64),
        storage_used_bytes: snap
            .as_ref()
            .and_then(|s| s.storage_used_bytes)
            .map(|v| v.max(0) as u64),
        proxy_bytes_in_total: proxy_in,
        proxy_bytes_out_total: proxy_out,
        display_topology,
        display_stream_capable,
    })
}

fn validate_version(version: &str) -> Result<(), Status> {
    validate_version_core(version).map_err(|e| Status::invalid_argument(e))
}

const CONNECTION_PROBE_MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024;
const CONNECTION_PROBE_MAX_DOWNLOAD_BYTES: usize = 4 * 1024 * 1024;
const CONNECTION_PROBE_DEFAULT_DOWNLOAD_BYTES: usize = 256 * 1024;

fn stack_apply_options_encoded_bytes(opts: &StackApplyOptions) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = opts.encode(&mut buf);
    buf
}

/// If metadata carries a digest, it must match the encoded last-chunk options.
fn verify_stack_apply_digest_matches(
    meta: &tonic::metadata::MetadataMap,
    opts: &Option<StackApplyOptions>,
) -> Result<(), Status> {
    let expected_hex = meta
        .get(META_STACK_APPLY_SHA256)
        .and_then(|v| v.to_str().ok());
    match opts {
        None => {
            if expected_hex.is_some() {
                return Err(Status::invalid_argument(
                    "x-deploy-stack-apply-sha256 set but no apply_options on last chunk",
                ));
            }
            Ok(())
        }
        Some(o) => {
            let buf = stack_apply_options_encoded_bytes(o);
            let digest = hex::encode(Sha256::digest(&buf));
            match expected_hex {
                None => Ok(()),
                Some(h) if h.trim() == digest => Ok(()),
                Some(_) => Err(Status::invalid_argument(
                    "stack apply options digest mismatch metadata",
                )),
            }
        }
    }
}

fn bundle_has_ui_static(bundle_root: &Path) -> bool {
    if bundle_root.join(".bundle-no-ui").exists() {
        return false;
    }
    bundle_root.join("share/ui/dist/index.html").is_file()
}

fn validate_stack_apply_transition(
    opts: &StackApplyOptions,
    bundle_root: &Path,
    host_dashboard: bool,
) -> Result<(), Status> {
    let mode = opts.mode;
    let has_ui = bundle_has_ui_static(bundle_root);
    match mode {
        x if x == StackApplyMode::Unspecified as i32 || x == StackApplyMode::None as i32 => Ok(()),
        x if x == StackApplyMode::EnableUi as i32 => {
            if !has_ui {
                return Err(Status::failed_precondition(
                    "StackApplyMode ENABLE_UI requires bundle with share/ui/dist (no .bundle-no-ui)",
                ));
            }
            if host_dashboard {
                return Err(Status::failed_precondition(
                    "host already has dashboard JWT; ENABLE_UI transition not applicable",
                ));
            }
            Ok(())
        }
        x if x == StackApplyMode::DisableUi as i32 => {
            if has_ui {
                return Err(Status::failed_precondition(
                    "StackApplyMode DISABLE_UI requires bundle without UI static (or .bundle-no-ui)",
                ));
            }
            if !host_dashboard {
                return Err(Status::failed_precondition(
                    "host has no dashboard JWT; DISABLE_UI transition not applicable",
                ));
            }
            Ok(())
        }
        _ => Err(Status::invalid_argument("unknown StackApplyMode")),
    }
}

/// For managed proxy sessions, wire secrets (VLESS/VMess UUID, Trojan password) must come from the
/// session row in the database, not only from `Open.wire_config_json`, so a valid session token
/// cannot be paired with a spoofed wire secret in the open payload.
fn apply_managed_wire_secrets(
    mut params: Option<wire_protocol::WireParams>,
    wire_mode: i32,
    managed: &Option<(GrpcProxySessionRow, crate::proxy_session::StoredPolicy)>,
) -> Option<wire_protocol::WireParams> {
    let Some(p) = params.as_mut() else {
        return params;
    };
    let Some((row, _)) = managed else {
        return params;
    };
    let Some(ref json) = row.wire_config_json else {
        return params;
    };
    let s = json.trim();
    if s.is_empty() {
        return params;
    }
    let Ok(session) = wire_protocol::WireParams::from_json(s) else {
        return params;
    };
    match wire_mode {
        1 => {
            if let Some(u) = session.uuid {
                let u = u.trim();
                if !u.is_empty() {
                    p.uuid = Some(u.to_string());
                }
            }
        }
        2 => {
            if let Some(pw) = session.password {
                if !pw.is_empty() {
                    p.password = Some(pw);
                }
            }
        }
        3 => {
            if let Some(u) = session.uuid {
                let u = u.trim();
                if !u.is_empty() {
                    p.uuid = Some(u.to_string());
                }
            }
        }
        4 => {
            if let Some(pw) = session.password {
                if !pw.is_empty() {
                    p.password = Some(pw);
                }
            }
            if let Some(m) = session.method {
                let m = m.trim();
                if !m.is_empty() {
                    p.method = Some(m.to_string());
                }
            }
        }
        5 => {
            if let Some(u) = session.username {
                let u = u.trim();
                if !u.is_empty() {
                    p.username = Some(u.to_string());
                }
            }
            if let Some(pw) = session.password {
                if !pw.is_empty() {
                    p.password = Some(pw);
                }
            }
        }
        _ => {}
    }
    params
}

fn validate_inbound_wire(
    wire_mode: Option<i32>,
    wire_config_json: Option<&str>,
) -> Result<(Option<i32>, Option<String>), Status> {
    use wire_protocol::WireParams;
    match (wire_mode, wire_config_json) {
        (None, None) => Ok((None, None)),
        (Some(m), Some(j)) => {
            let j = j.trim();
            if j.is_empty() {
                return Err(Status::invalid_argument("wire_config_json is empty"));
            }
            if m != 1 && m != 2 && m != 3 && m != 4 && m != 5 {
                return Err(Status::invalid_argument(
                    "wire_mode must be 1 (VLESS), 2 (Trojan), 3 (VMess), 4 (Shadowsocks), or 5 (SOCKS5)",
                ));
            }
            let p = WireParams::from_json(j)
                .map_err(|e| Status::invalid_argument(format!("wire_config_json: {e}")))?;
            match m {
                1 | 3 => {
                    if p.uuid.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                        return Err(Status::invalid_argument(
                            "uuid is required in wire_config_json for VLESS and VMess",
                        ));
                    }
                }
                2 => {
                    if p.password.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                        return Err(Status::invalid_argument(
                            "password is required in wire_config_json for Trojan",
                        ));
                    }
                }
                4 => {
                    if p.password.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                        return Err(Status::invalid_argument(
                            "password is required in wire_config_json for Shadowsocks",
                        ));
                    }
                    if p.method.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                        return Err(Status::invalid_argument(
                            "method is required in wire_config_json for Shadowsocks",
                        ));
                    }
                }
                5 => {}
                _ => {}
            }
            Ok((Some(m), Some(j.to_string())))
        }
        _ => Err(Status::invalid_argument(
            "wire_mode and wire_config_json must both be set or both omitted",
        )),
    }
}

#[derive(Clone)]
struct IngressDb {
    protocol: Option<i16>,
    listen_tcp: Option<i32>,
    listen_udp: Option<i32>,
    config_json: Option<String>,
    tls_json: Option<String>,
    template_version: i32,
}

impl IngressDb {
    fn disabled() -> Self {
        Self {
            protocol: None,
            listen_tcp: None,
            listen_udp: None,
            config_json: None,
            tls_json: None,
            template_version: 1,
        }
    }
}

fn validate_ingress_create(inner: &CreateConnectionRequest) -> Result<IngressDb, Status> {
    let has_any = inner.ingress_protocol.is_some()
        || inner.ingress_listen_port.is_some()
        || inner.ingress_listen_udp_port.is_some()
        || inner
            .ingress_config_json
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        || inner
            .ingress_tls_json
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        || inner.ingress_template_version.is_some();
    let proto = inner.ingress_protocol.unwrap_or(0);
    if !has_any && proto == 0 {
        return Ok(IngressDb::disabled());
    }
    if proto < 1 || proto > 6 {
        return Err(Status::invalid_argument(
            "ingress_protocol must be 0 (disabled) or 1=VLESS .. 6=Hysteria2",
        ));
    }
    let port = inner.ingress_listen_port.ok_or_else(|| {
        Status::invalid_argument("ingress_listen_port is required when ingress is enabled")
    })? as i32;
    if port <= 0 || port > 65535 {
        return Err(Status::invalid_argument("invalid ingress_listen_port"));
    }
    let cfg = inner
        .ingress_config_json
        .as_deref()
        .unwrap_or("")
        .trim();
    if cfg.is_empty() {
        return Err(Status::invalid_argument(
            "ingress_config_json is required when ingress is enabled",
        ));
    }
    serde_json::from_str::<serde_json::Value>(cfg).map_err(|e| {
        Status::invalid_argument(format!("ingress_config_json is not valid JSON: {e}"))
    })?;
    let tls_s = match inner.ingress_tls_json.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(t) => {
            serde_json::from_str::<serde_json::Value>(t).map_err(|e| {
                Status::invalid_argument(format!("ingress_tls_json is not valid JSON: {e}"))
            })?;
            Some(t.to_string())
        }
    };
    let udp = match inner.ingress_listen_udp_port {
        None | Some(0) => None,
        Some(p) => {
            let p = p as i32;
            if p <= 0 || p > 65535 {
                return Err(Status::invalid_argument("invalid ingress_listen_udp_port"));
            }
            Some(p)
        }
    };
    let ver = inner
        .ingress_template_version
        .unwrap_or(1)
        .max(1) as i32;
    Ok(IngressDb {
        protocol: Some(proto as i16),
        listen_tcp: Some(port),
        listen_udp: udp,
        config_json: Some(cfg.to_string()),
        tls_json: tls_s,
        template_version: ver,
    })
}

fn should_update_ingress(inner: &UpdateProxySettingsRequest) -> bool {
    inner.ingress_protocol.is_some()
        || inner.ingress_listen_port.is_some()
        || inner.ingress_listen_udp_port.is_some()
        || inner
            .ingress_config_json
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        || inner
            .ingress_tls_json
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        || inner.ingress_template_version.is_some()
}

fn validate_ingress_update(inner: &UpdateProxySettingsRequest) -> Result<IngressDb, Status> {
    let proto = inner.ingress_protocol.unwrap_or(0);
    if proto == 0 {
        return Ok(IngressDb::disabled());
    }
    if proto < 1 || proto > 6 {
        return Err(Status::invalid_argument(
            "ingress_protocol must be 0 (clear) or 1..6",
        ));
    }
    let port = inner.ingress_listen_port.ok_or_else(|| {
        Status::invalid_argument("ingress_listen_port is required when updating ingress")
    })? as i32;
    if port <= 0 || port > 65535 {
        return Err(Status::invalid_argument("invalid ingress_listen_port"));
    }
    let cfg = inner
        .ingress_config_json
        .as_deref()
        .ok_or_else(|| {
            Status::invalid_argument("ingress_config_json is required when updating ingress")
        })?
        .trim();
    if cfg.is_empty() {
        return Err(Status::invalid_argument(
            "ingress_config_json is required when updating ingress",
        ));
    }
    serde_json::from_str::<serde_json::Value>(cfg).map_err(|e| {
        Status::invalid_argument(format!("ingress_config_json is not valid JSON: {e}"))
    })?;
    let tls_s = match inner.ingress_tls_json.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(t) => {
            serde_json::from_str::<serde_json::Value>(t).map_err(|e| {
                Status::invalid_argument(format!("ingress_tls_json is not valid JSON: {e}"))
            })?;
            Some(t.to_string())
        }
    };
    let udp = match inner.ingress_listen_udp_port {
        None | Some(0) => None,
        Some(p) => {
            let p = p as i32;
            if p <= 0 || p > 65535 {
                return Err(Status::invalid_argument("invalid ingress_listen_udp_port"));
            }
            Some(p)
        }
    };
    let ver = inner
        .ingress_template_version
        .unwrap_or(1)
        .max(1) as i32;
    Ok(IngressDb {
        protocol: Some(proto as i16),
        listen_tcp: Some(port),
        listen_udp: udp,
        config_json: Some(cfg.to_string()),
        tls_json: tls_s,
        template_version: ver,
    })
}

fn expires_ms_from_policy_json(
    policy_json: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> i64 {
    if let Ok(p) = proxy_session::parse_policy_json(policy_json) {
        if p.never_expires || p.max_session_duration_sec.is_none() {
            return -1;
        }
    }
    expires_at.timestamp_millis()
}

#[derive(Clone)]
pub struct DeployServiceImpl {
    /// Base path; each project uses [`project_deploy_root`] under this path.
    pub base_root: PathBuf,
    pub max_upload_bytes: u64,
    pub max_server_stack_bytes: u64,
    pub allow_server_stack_update: bool,
    pub binary_fallback: String,
    /// gRPC endpoint URL for local client bundles (`DEPLOY_GRPC_PUBLIC_URL` / default).
    pub public_url: String,
    pub states: Arc<tokio::sync::Mutex<HashMap<String, AppState>>>,
    pub db: Option<Arc<DbStore>>,
    pub auth: Option<Arc<ServerAuth>>,
    /// Previous network counters for host stats / network detail (same process as control-api).
    pub host_net: Arc<std::sync::Mutex<Option<NetCounters>>>,
    /// Optional app log path for `log_tail` in host stats (`DEPLOY_HOST_STATS_LOG_TAIL`).
    pub log_tail_path: Option<PathBuf>,
    pub session_hub: Arc<SessionAuditHub>,
    pub proxy_metrics: Arc<ProxyTunnelMetrics>,
    pub tunnel_admission: Arc<TunnelAdmission>,
    pub tunnel_redis: Option<Arc<TunnelRedis>>,
    /// QUIC UDP data-plane (optional; tickets issued from raw `ProxyTunnel` when enabled).
    pub quic_dataplane: Option<crate::quic::QuicDataplaneState>,
}

impl DeployServiceImpl {
    pub fn new(
        base_root: PathBuf,
        max_upload_bytes: u64,
        max_server_stack_bytes: u64,
        allow_server_stack_update: bool,
        binary_fallback: String,
        public_url: String,
        states: Arc<tokio::sync::Mutex<HashMap<String, AppState>>>,
        db: Option<Arc<DbStore>>,
        auth: Option<Arc<ServerAuth>>,
        host_net: Arc<std::sync::Mutex<Option<NetCounters>>>,
        log_tail_path: Option<PathBuf>,
        session_hub: Arc<SessionAuditHub>,
        proxy_metrics: Arc<ProxyTunnelMetrics>,
        tunnel_admission: Arc<TunnelAdmission>,
        tunnel_redis: Option<Arc<TunnelRedis>>,
        quic_dataplane: Option<crate::quic::QuicDataplaneState>,
    ) -> Self {
        Self {
            base_root,
            max_upload_bytes,
            max_server_stack_bytes,
            allow_server_stack_update,
            binary_fallback,
            public_url,
            states,
            db,
            auth,
            host_net,
            log_tail_path,
            session_hub,
            proxy_metrics,
            tunnel_admission,
            tunnel_redis,
            quic_dataplane,
        }
    }

    fn status_response(&self, current_version: String, state: String) -> StatusResponse {
        let (client_connect_token, client_connect_pairing) = if let Some(ref auth) = self.auth {
            (
                auth.server_pubkey_b64.clone(),
                auth.pairing_code.read().clone(),
            )
        } else {
            (String::new(), String::new())
        };
        StatusResponse {
            current_version,
            state,
            client_connect_token,
            client_connect_url: self.public_url.clone(),
            client_connect_pairing,
        }
    }

    fn staging_dir(project_root: &Path) -> PathBuf {
        project_root.join(".staging")
    }

    fn spawn_db_record(
        &self,
        project_id: &str,
        kind: &'static str,
        deployed_version: &str,
        current_version: &str,
        state: &str,
        last_error: Option<&str>,
    ) {
        let Some(db) = self.db.clone() else {
            return;
        };
        let pid = project_id.to_string();
        let deployed_version = deployed_version.to_string();
        let current_version = current_version.to_string();
        let state = state.to_string();
        let last_err = last_error.map(|s| s.to_string());
        let snapshot = format!("{state}|{current_version}|{:?}", last_err);
        tokio::spawn(async move {
            if let Err(e) = db
                .record_event(&pid, kind, &deployed_version, Some(&snapshot))
                .await
            {
                error!(%e, "deploy_db record_event");
            }
            if let Err(e) = db
                .upsert_snapshot(&pid, &current_version, &state, last_err.as_deref())
                .await
            {
                error!(%e, "deploy_db upsert_snapshot");
            }
        });
    }
}

#[cfg(unix)]
fn set_current_symlink(root: &Path, version: &str) -> std::io::Result<()> {
    use std::os::unix::fs::symlink;
    let current = root.join("current");
    let rel = Path::new("releases").join(version);
    let tmp = root.join(".current.tmp");
    let _ = std::fs::remove_file(&tmp);
    symlink(&rel, &tmp)?;
    std::fs::rename(&tmp, &current)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_current_symlink(_root: &Path, _version: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink deploy only supported on Unix",
    ))
}

#[cfg(unix)]
fn ensure_run_sh_executable(release_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let run = release_dir.join("run.sh");
    if run.exists() {
        if let Ok(meta) = std::fs::metadata(&run) {
            let mut p = meta.permissions();
            p.set_mode(0o755);
            let _ = std::fs::set_permissions(&run, p);
        }
    }
}

#[cfg(not(unix))]
fn ensure_run_sh_executable(_release_dir: &Path) {}

fn unpack_tar_gz(src: &Path, dst: &Path) -> std::io::Result<()> {
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

async fn spawn_release(
    root: &Path,
    version: &str,
    binary_fallback: &str,
) -> Result<tokio::process::Child, Status> {
    let release_dir = release_dir_for_version(root, version);
    let run_sh = release_dir.join("run.sh");

    let mut cmd = if run_sh.exists() {
        let mut c = tokio::process::Command::new("sh");
        c.arg(run_sh.as_os_str())
            .current_dir(&release_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        c
    } else {
        let bin = release_dir.join(binary_fallback);
        if !bin.exists() {
            return Err(Status::failed_precondition(format!(
                "neither run.sh nor {}/{} found",
                release_dir.display(),
                binary_fallback
            )));
        }
        let mut c = tokio::process::Command::new(&bin);
        c.current_dir(&release_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        c
    };

    cmd.spawn()
        .map_err(|e| Status::internal(format!("spawn failed: {e}")))
}

async fn stop_child(child: &mut tokio::process::Child) {
    if let Err(e) = child.kill().await {
        warn!(error = %e, "kill child");
    }
    let _ = child.wait().await;
}

#[tonic::async_trait]
impl DeployService for DeployServiceImpl {
    type ProxyTunnelStream =
        Pin<Box<dyn Stream<Item = Result<ProxyServerMsg, Status>> + Send + 'static>>;

    async fn pair(
        &self,
        request: Request<PairRequest>,
    ) -> Result<Response<PairResponse>, Status> {
        let peer_ip = peer_ip_from_request(&request);
        let pubkey_slot = request
            .extensions()
            .get::<crate::session_audit::AuditedConnectInfo>()
            .map(|c| c.pubkey_slot.clone());
        let auth = match self.auth.as_ref() {
            Some(a) => a,
            None => {
                self.session_hub.log_pair_outcome(
                    false,
                    &peer_ip,
                    None,
                    "authentication disabled; pairing unavailable",
                );
                return Err(Status::failed_precondition(
                    "authentication disabled; pairing unavailable",
                ));
            }
        };
        let _ = auth
            .reload_pairing_code()
            .map_err(|e| Status::internal(e.to_string()))?;
        let r = request.into_inner();
        if r.client_public_key_b64.is_empty()
            || r.nonce.is_empty()
            || r.client_signature_b64.is_empty()
        {
            self.session_hub
                .log_pair_outcome(false, &peer_ip, None, "missing pair fields");
            return Err(Status::invalid_argument("missing pair fields"));
        }
        let now = now_unix_ms();
        if (now - r.timestamp_ms).abs() > auth.config.max_clock_skew_ms {
            self.session_hub.log_pair_outcome(
                false,
                &peer_ip,
                Some(&r.client_public_key_b64),
                "timestamp skew",
            );
            return Err(Status::deadline_exceeded("timestamp skew"));
        }
        if let Err(e) = auth.verify_pairing(&r.pairing_code) {
            self.session_hub.log_pair_outcome(
                false,
                &peer_ip,
                Some(&r.client_public_key_b64),
                "invalid pairing code",
            );
            return Err(e);
        }
        if let Err(e) = verify_pair_signature(
            &r.client_public_key_b64,
            &auth.server_pubkey_b64,
            r.timestamp_ms,
            &r.nonce,
            &r.pairing_code,
            &r.client_signature_b64,
        ) {
            self.session_hub.log_pair_outcome(
                false,
                &peer_ip,
                Some(&r.client_public_key_b64),
                "bad pair signature",
            );
            return Err(e);
        }
        auth.add_peer(&r.client_public_key_b64)?;
        if let Some(slot) = pubkey_slot {
            *slot.lock() = Some(r.client_public_key_b64.clone());
        }
        self.session_hub.log_pair_outcome(
            true,
            &peer_ip,
            Some(&r.client_public_key_b64),
            "paired",
        );
        let server_sig = sign_pair_response(
            &auth.signing_key,
            &auth.server_pubkey_b64,
            &r.client_public_key_b64,
            r.timestamp_ms,
            &r.nonce,
        );
        Ok(Response::new(PairResponse {
            server_public_key_b64: auth.server_pubkey_b64.clone(),
            server_signature_b64: server_sig,
            status: "paired".to_string(),
        }))
    }

    async fn upload(
        &self,
        request: Request<Streaming<DeployChunk>>,
    ) -> Result<Response<DeployResponse>, Status> {
        let meta = request.metadata().clone();
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_upload_metadata(&meta, &peers, &auth.config, &auth.nonce_tracker)
                .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let expected_version = meta
            .get(META_VERSION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let mut stream = request.into_inner();

        let mut version: Option<String> = None;
        let mut project_key: Option<String> = None;
        let mut project_root: Option<PathBuf> = None;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut expected_sha_hex: Option<String> = None;
        let mut temp_path: Option<PathBuf> = None;
        let mut file: Option<tokio::fs::File> = None;

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| Status::internal(e.to_string()))?;

            if version.is_none() {
                if chunk.version.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument("first chunk must set version"));
                }
                if let Some(ref ev) = expected_version {
                    if chunk.version != *ev {
                        if let Some(ref p) = temp_path {
                            let _ = tokio::fs::remove_file(p).await;
                        }
                        return Err(Status::invalid_argument(
                            "first chunk version must match x-deploy-version metadata",
                        ));
                    }
                }
                validate_version(&chunk.version)?;
                validate_project_id(&chunk.project_id).map_err(Status::invalid_argument)?;
                let chunk_proj = normalize_project_id(&chunk.project_id);
                let meta_proj = meta.get(META_PROJECT).and_then(|v| v.to_str().ok());
                match meta_proj {
                    Some(m) => {
                        if normalize_project_id(m) != chunk_proj {
                            return Err(Status::invalid_argument(
                                "project_id mismatch between metadata and first chunk",
                            ));
                        }
                    }
                    None => {
                        if chunk_proj != "default" {
                            return Err(Status::invalid_argument(
                                "non-default project_id requires x-deploy-project metadata",
                            ));
                        }
                    }
                }

                let root = project_deploy_root(&self.base_root, &chunk.project_id);
                tokio::fs::create_dir_all(Self::staging_dir(&root))
                    .await
                    .map_err(|e| Status::internal(format!("staging dir: {e}")))?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let tp = Self::staging_dir(&root).join(format!("upload_{stamp}.tar.gz"));
                let f = tokio::fs::File::create(&tp)
                    .await
                    .map_err(|e| Status::internal(format!("temp file: {e}")))?;
                file = Some(f);
                temp_path = Some(tp);
                project_root = Some(root);
                project_key = Some(chunk_proj);
                version = Some(chunk.version.clone());
            } else if !chunk.version.is_empty()
                && chunk.version.as_str() != version.as_deref().unwrap()
            {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::invalid_argument("version mismatch between chunks"));
            }

            let n = chunk.data.len() as u64;
            if written.saturating_add(n) > self.max_upload_bytes {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::resource_exhausted(format!(
                    "artifact exceeds limit of {} bytes",
                    self.max_upload_bytes
                )));
            }

            hasher.update(&chunk.data);
            file.as_mut()
                .ok_or_else(|| Status::internal("internal: file not open"))?
                .write_all(&chunk.data)
                .await
                .map_err(|e| Status::internal(format!("write: {e}")))?;
            written = written.saturating_add(n);

            if chunk.is_last {
                if chunk.sha256_hex.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument(
                        "sha256_hex required when is_last is true",
                    ));
                }
                expected_sha_hex = Some(chunk.sha256_hex.clone());
            }
        }

        let version = version.ok_or_else(|| {
            if let Some(ref p) = temp_path {
                let _ = std::fs::remove_file(p);
            }
            Status::invalid_argument("no version in stream")
        })?;
        let project_key = project_key.ok_or_else(|| Status::invalid_argument("no project in stream"))?;
        let project_root = project_root.ok_or_else(|| Status::internal("internal: no project root"))?;
        let temp_path = temp_path.ok_or_else(|| Status::internal("internal: no temp path"))?;

        let expected_hex = expected_sha_hex.ok_or_else(|| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("stream must end with is_last=true and sha256_hex")
        })?;

        let digest = hasher.finalize();
        let expected_bytes = hex::decode(expected_hex.trim()).map_err(|_| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("invalid sha256_hex (not hex)")
        })?;
        if expected_bytes.len() != 32 {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("sha256 must be 32 bytes"));
        }
        if digest.as_slice() != expected_bytes.as_slice() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("SHA-256 mismatch"));
        }

        let release_dir = release_dir_for_version(&project_root, &version);
        tokio::fs::create_dir_all(deploy_core::releases_dir(&project_root))
            .await
            .map_err(|e| Status::internal(format!("releases dir: {e}")))?;

        let tp = temp_path.clone();
        let rd = release_dir.clone();
        tokio::task::spawn_blocking(move || {
            if rd.exists() {
                std::fs::remove_dir_all(&rd).map_err(|e| e.to_string())?;
            }
            std::fs::create_dir_all(&rd).map_err(|e| e.to_string())?;
            unpack_tar_gz(&tp, &rd).map_err(|e| e.to_string())?;
            ensure_run_sh_executable(&rd);
            std::fs::remove_file(&tp).map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?
        .map_err(|e| Status::internal(e))?;

        info!(project = %project_key, version = %version, "artifact unpacked");

        let root = project_root.clone();
        let bf = self.binary_fallback.clone();
        let mut map = self.states.lock().await;
        let st = map.entry(project_key.clone()).or_insert_with(AppState::default);

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }

        if let Err(e) = set_current_symlink(&root, &version) {
            error!(error = %e, "symlink");
            st.state = "error".to_string();
            st.last_error = Some(e.to_string());
            return Err(Status::internal(format!("symlink: {e}")));
        }

        match spawn_release(&root, &version, &bf).await {
            Ok(child) => {
                st.child = Some(child);
                st.current_version = version.clone();
                st.state = "running".to_string();
                st.last_error = None;
                info!(project = %project_key, version = %version, "deployed and started");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(map);
                self.spawn_db_record(
                    &project_key,
                    "upload",
                    &version,
                    &cur,
                    &state,
                    err.as_deref(),
                );
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.message().to_string());
                return Err(e);
            }
        }

        Ok(Response::new(DeployResponse {
            status: "ok".to_string(),
            deployed_version: version,
        }))
    }

    async fn get_status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("GetStatus", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetStatus",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();
        let key = normalize_project_id(&inner.project_id);
        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);
        refresh_process_state(st);

        let current = status_current_version_display(
            &st.current_version,
            &root,
            env!("CARGO_PKG_VERSION"),
        );

        Ok(Response::new(
            self.status_response(current, st.state.clone()),
        ))
    }

    async fn rollback(
        &self,
        request: Request<RollbackRequest>,
    ) -> Result<Response<RollbackResponse>, Status> {
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let v = request.get_ref().version.clone();
        let sign_payload = signing_payload("Rollback", &request.get_ref().project_id, &v);
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "Rollback",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();
        let key = normalize_project_id(&inner.project_id);
        validate_version(&v)?;

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let target = release_dir_for_version(&root, &v);
        if !target.is_dir() {
            return Err(Status::not_found(format!("release {v} not found")));
        }

        let bf = self.binary_fallback.clone();
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }

        if let Err(e) = set_current_symlink(&root, &v) {
            error!(error = %e, "rollback symlink");
            st.state = "error".to_string();
            st.last_error = Some(e.to_string());
            return Err(Status::internal(format!("symlink: {e}")));
        }

        match spawn_release(&root, &v, &bf).await {
            Ok(child) => {
                st.child = Some(child);
                st.current_version = v.clone();
                st.state = "running".to_string();
                st.last_error = None;
                info!(project = %key, version = %v, "rollback complete");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(map);
                self.spawn_db_record(&key, "rollback", &v, &cur, &state, err.as_deref());
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.message().to_string());
                return Err(e);
            }
        }

        Ok(Response::new(RollbackResponse {
            status: "ok".to_string(),
            active_version: v,
        }))
    }

    async fn stop_process(
        &self,
        request: Request<StopProcessRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("StopProcess", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "StopProcess",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();
        let key = normalize_project_id(&inner.project_id);

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);
        refresh_process_state(st);

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }
        st.state = "stopped".to_string();
        st.last_error = None;

        let mut current = st.current_version.clone();
        if current.is_empty() {
            if let Some(v) = read_current_version_from_symlink(&root) {
                current = v;
                st.current_version = current.clone();
            }
        }

        let cur = st.current_version.clone();
        let state = st.state.clone();
        drop(map);
        self.spawn_db_record(&key, "stop", &cur, &cur, &state, None);

        let for_response = status_current_version_display(
            &cur,
            &root,
            env!("CARGO_PKG_VERSION"),
        );
        Ok(Response::new(
            self.status_response(for_response, "stopped".to_string()),
        ))
    }

    async fn restart_process(
        &self,
        request: Request<RestartProcessRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("RestartProcess", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "RestartProcess",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();
        let key = normalize_project_id(&inner.project_id);

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let bf = self.binary_fallback.clone();
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);
        refresh_process_state(st);

        let mut ver = st.current_version.clone();
        if ver.is_empty() {
            ver = read_current_version_from_symlink(&root).unwrap_or_default();
        }
        if ver.is_empty() {
            return Err(Status::failed_precondition(
                "no active release; deploy or rollback first",
            ));
        }

        let target = release_dir_for_version(&root, &ver);
        if !target.is_dir() {
            return Err(Status::failed_precondition(format!(
                "release directory for {ver} missing"
            )));
        }

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }

        match spawn_release(&root, &ver, &bf).await {
            Ok(child) => {
                st.child = Some(child);
                st.current_version = ver.clone();
                st.state = "running".to_string();
                st.last_error = None;
                info!(project = %key, version = %ver, "process restarted");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(map);
                self.spawn_db_record(&key, "restart", &ver, &cur, &state, err.as_deref());
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.message().to_string());
                return Err(e);
            }
        }

        let map = self.states.lock().await;
        let st = map.get(&key).ok_or_else(|| Status::internal("internal: project state"))?;
        Ok(Response::new(self.status_response(
            st.current_version.clone(),
            st.state.clone(),
        )))
    }

    async fn get_host_stats(
        &self,
        request: Request<HostStatsRequest>,
    ) -> Result<Response<HostStatsResponse>, Status> {
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("GetHostStats", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetHostStats",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let host_net = self.host_net.clone();
        let log_tail = self.log_tail_path.clone();

        let view = tokio::task::spawn_blocking(move || {
            let prev = host_net.lock().unwrap().clone();
            let (stats, net) =
                collect_host_stats(&root, prev.as_ref(), log_tail.as_deref());
            *host_net.lock().unwrap() = Some(net);
            stats
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(host_stats_view_to_proto(&view)))
    }

    async fn get_host_stats_detail(
        &self,
        request: Request<HostStatsDetailRequest>,
    ) -> Result<Response<HostStatsDetailResponse>, Status> {
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("GetHostStatsDetail", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetHostStatsDetail",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();

        if inner.kind == HostStatsDetailKind::Unspecified as i32 {
            return Err(Status::invalid_argument("kind is required"));
        }

        let top = inner.top.clamp(5, 100) as usize;
        let limit = inner.limit.clamp(10, 2000) as usize;
        let q = inner.q.clone();
        let kind = inner.kind;

        let host_net = self.host_net.clone();

        let detail = tokio::task::spawn_blocking(move || match kind {
            k if k == HostStatsDetailKind::HostStatsDetailCpu as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Cpu(cpu_detail_to_proto(
                    &collect_cpu_detail(top),
                ))),
            },
            k if k == HostStatsDetailKind::HostStatsDetailMemory as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Memory(memory_detail_to_proto(
                    &collect_memory_detail(top),
                ))),
            },
            k if k == HostStatsDetailKind::HostStatsDetailDisk as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Disk(disk_detail_to_proto(
                    &collect_disk_detail(top),
                ))),
            },
            k if k == HostStatsDetailKind::HostStatsDetailNetwork as i32 => {
                let prev = host_net.lock().unwrap().clone();
                let (d, net) = collect_network_detail(prev.as_ref());
                *host_net.lock().unwrap() = Some(net);
                HostStatsDetailResponse {
                    detail: Some(HostStatsDetailOneof::Network(network_detail_to_proto(&d))),
                }
            }
            k if k == HostStatsDetailKind::HostStatsDetailProcesses as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Processes(processes_detail_to_proto(
                    &collect_processes_list(&q, limit),
                ))),
            },
            _ => HostStatsDetailResponse { detail: None },
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        if detail.detail.is_none() {
            return Err(Status::invalid_argument("invalid kind"));
        }

        Ok(Response::new(detail))
    }

    async fn upload_server_stack(
        &self,
        request: Request<Streaming<ServerStackChunk>>,
    ) -> Result<Response<ServerStackResponse>, Status> {
        if !self.allow_server_stack_update {
            return Err(Status::failed_precondition(
                "server stack OTA disabled; set DEPLOY_ALLOW_SERVER_STACK_UPDATE=1 on deploy-server",
            ));
        }
        let meta = request.metadata().clone();
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_upload_server_stack_metadata(&meta, &peers, &auth.config, &auth.nonce_tracker)
                .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let expected_version = meta
            .get(META_VERSION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let mut stream = request.into_inner();
        let mut version: Option<String> = None;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut expected_sha_hex: Option<String> = None;
        let mut temp_path: Option<PathBuf> = None;
        let mut file: Option<tokio::fs::File> = None;
        let mut last_apply_options: Option<StackApplyOptions> = None;

        let staging_base = self.base_root.join(".stack-staging");
        tokio::fs::create_dir_all(&staging_base)
            .await
            .map_err(|e| Status::internal(format!("stack staging: {e}")))?;

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| Status::internal(e.to_string()))?;

            if version.is_none() {
                if chunk.version.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument("first chunk must set version"));
                }
                if let Some(ref ev) = expected_version {
                    if chunk.version != *ev {
                        if let Some(ref p) = temp_path {
                            let _ = tokio::fs::remove_file(p).await;
                        }
                        return Err(Status::invalid_argument(
                            "first chunk version must match x-deploy-version metadata",
                        ));
                    }
                }
                validate_version(&chunk.version)?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let tp = staging_base.join(format!("stack_upload_{stamp}.tar.gz"));
                let f = tokio::fs::File::create(&tp)
                    .await
                    .map_err(|e| Status::internal(format!("temp file: {e}")))?;
                file = Some(f);
                temp_path = Some(tp);
                version = Some(chunk.version.clone());
            } else if !chunk.version.is_empty()
                && chunk.version.as_str() != version.as_deref().unwrap()
            {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::invalid_argument("version mismatch between chunks"));
            }

            let n = chunk.data.len() as u64;
            if written.saturating_add(n) > self.max_server_stack_bytes {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::resource_exhausted(format!(
                    "server stack artifact exceeds limit of {} bytes",
                    self.max_server_stack_bytes
                )));
            }

            hasher.update(&chunk.data);
            file.as_mut()
                .ok_or_else(|| Status::internal("internal: file not open"))?
                .write_all(&chunk.data)
                .await
                .map_err(|e| Status::internal(format!("write: {e}")))?;
            written = written.saturating_add(n);

            if chunk.is_last {
                if chunk.sha256_hex.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument(
                        "sha256_hex required when is_last is true",
                    ));
                }
                expected_sha_hex = Some(chunk.sha256_hex.clone());
                last_apply_options = chunk.apply_options.clone();
            }
        }

        verify_stack_apply_digest_matches(&meta, &last_apply_options)?;

        let version = version.ok_or_else(|| {
            if let Some(ref p) = temp_path {
                let _ = std::fs::remove_file(p);
            }
            Status::invalid_argument("no version in stream")
        })?;
        let temp_path = temp_path.ok_or_else(|| Status::internal("internal: no temp path"))?;
        let expected_hex = expected_sha_hex.ok_or_else(|| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("stream must end with is_last=true and sha256_hex")
        })?;

        let digest = hasher.finalize();
        let expected_bytes = hex::decode(expected_hex.trim()).map_err(|_| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("invalid sha256_hex (not hex)")
        })?;
        if expected_bytes.len() != 32 {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("sha256 must be 32 bytes"));
        }
        if digest.as_slice() != expected_bytes.as_slice() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("SHA-256 mismatch"));
        }

        let extract_dir = staging_base.join(format!("extract_{}", version.replace(['/', '\\'], "_")));
        if extract_dir.exists() {
            let _ = tokio::fs::remove_dir_all(&extract_dir).await;
        }
        tokio::fs::create_dir_all(&extract_dir)
            .await
            .map_err(|e| Status::internal(format!("extract dir: {e}")))?;

        let tp = temp_path.clone();
        let ed = extract_dir.clone();
        tokio::task::spawn_blocking(move || unpack_tar_gz(&tp, &ed))
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .map_err(|e| Status::internal(e.to_string()))?;

        let bundle_root = match find_pirate_bundle_root(&extract_dir) {
            Ok(p) => p,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                let _ = tokio::fs::remove_dir_all(&extract_dir).await;
                return Err(e);
            }
        };

        let manifest_path = bundle_root.join("server-stack-manifest.json");
        let (ds_ver, ca_ver) = if manifest_path.exists() {
            match std::fs::read_to_string(&manifest_path) {
                Ok(s) => parse_stack_manifest_versions(&s),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

        let (host_dashboard, host_nginx) = read_host_stack_ui_flags();
        let transition_apply = last_apply_options.as_ref().is_some_and(|o| {
            o.mode == StackApplyMode::EnableUi as i32 || o.mode == StackApplyMode::DisableUi as i32
        });
        if transition_apply {
            if let Some(ref o) = last_apply_options {
                validate_stack_apply_transition(o, &bundle_root, host_dashboard)?;
            }
        }

        let apply_json_path: Option<PathBuf> = if transition_apply {
            let o = last_apply_options.as_ref().ok_or_else(|| {
                Status::internal("internal: transition apply without options")
            })?;
            let mode_str = if o.mode == StackApplyMode::EnableUi as i32 {
                "enable_ui"
            } else {
                "disable_ui"
            };
            let val = serde_json::json!({
                "mode": mode_str,
                "domain": o.domain,
                "ui_admin_username": o.ui_admin_username,
                "ui_admin_password": o.ui_admin_password,
                "install_nginx": o.install_nginx,
                "deploy_allow_server_stack_update": o.deploy_allow_server_stack_update,
                "control_api_host_stats_series": o.control_api_host_stats_series,
                "control_api_host_stats_stream": o.control_api_host_stats_stream,
                "nginx_keep_api_proxy": o.nginx_keep_api_proxy,
                "host_nginx_pirate_site": host_nginx,
            });
            let jp = staging_base.join(format!("apply_{}.json", version.replace(['/', '\\'], "_")));
            tokio::fs::write(&jp, val.to_string())
                .await
                .map_err(|e| Status::internal(format!("write apply json: {e}")))?;
            #[cfg(unix)]
            tokio::fs::set_permissions(&jp, PermissionsExt::from_mode(0o600))
                .await
                .map_err(|e| Status::internal(format!("chmod apply json: {e}")))?;
            Some(jp)
        } else {
            None
        };

        let br = bundle_root.clone();
        let ver = version.clone();
        let status = apply_stack_bundle_command(&br, &ver, apply_json_path.as_ref()).await;

        let _ = tokio::fs::remove_file(&temp_path).await;
        if let Some(ref jp) = apply_json_path {
            let _ = tokio::fs::remove_file(jp).await;
        }
        let _ = tokio::fs::remove_dir_all(&extract_dir).await;

        match status {
            Ok(s) if s.success() => {
                info!(version = %ver, "server stack OTA applied");
                Ok(Response::new(ServerStackResponse {
                    status: "ok".to_string(),
                    applied_version: ver.clone(),
                    deploy_server_pkg_version: ds_ver,
                    control_api_pkg_version: ca_ver,
                }))
            }
            Ok(s) => Err(Status::internal(format!(
                "pirate-apply-stack-bundle exited with {}",
                s.code().unwrap_or(-1)
            ))),
            Err(e) => Err(Status::internal(e)),
        }
    }

    async fn get_server_stack_info(
        &self,
        request: Request<ServerStackInfoRequest>,
    ) -> Result<Response<ServerStackInfo>, Status> {
        let meta = request.metadata().clone();
        let sign_payload = signing_payload("GetServerStackInfo", "", "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetServerStackInfo",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }

        let root = PathBuf::from(deploy_core::PIRATE_VAR_LIB);
        let bundle_version = read_server_stack_bundle_version_from_var_lib().unwrap_or_default();

        let manifest_path = root.join("server-stack-manifest.json");
        let manifest_json = if manifest_path.exists() {
            tokio::fs::read_to_string(&manifest_path)
                .await
                .ok()
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        };

        let deploy_server_binary_version = Some(env!("CARGO_PKG_VERSION").to_string());

        let (host_dashboard_enabled, host_nginx_pirate_site) = read_host_stack_ui_flags();

        let gui_path = root.join(deploy_core::HOST_GUI_INSTALL_JSON);
        let (host_gui_detected_at_install, host_gui_install_json) =
            if gui_path.is_file() {
                match tokio::fs::read_to_string(&gui_path).await {
                    Ok(raw) => {
                        let det = deploy_core::host_gui_detected_from_install_json(&raw);
                        (det, Some(raw))
                    }
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };

        Ok(Response::new(ServerStackInfo {
            bundle_version,
            manifest_json,
            deploy_server_binary_version,
            host_dashboard_enabled,
            host_nginx_pirate_site,
            host_gui_detected_at_install,
            host_gui_install_json,
        }))
    }

    async fn proxy_tunnel(
        &self,
        request: Request<Streaming<ProxyClientMsg>>,
    ) -> Result<Response<Self::ProxyTunnelStream>, Status> {
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; ProxyTunnel unavailable")
        })?;
        let meta = request.metadata().clone();
        let sign_payload = signing_payload("ProxyTunnel", "default", "");
        {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "ProxyTunnel",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }

        let mut inbound = request.into_inner();
        let first = inbound
            .message()
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::invalid_argument("empty client stream"))?;

        let open = match first.body {
            Some(proxy_client_msg::Body::Open(o)) => o,
            _ => {
                return Err(Status::invalid_argument(
                    "first ProxyClientMsg must be Open { host, port }",
                ));
            }
        };

        // deploy_proto::ProxyWireMode: 0 = RAW_TCP_RELAY, 1 = VLESS, 2 = TROJAN, 3 = VMESS
        let wire_mode = open.wire_mode.unwrap_or(0);
        let wire_json = open.wire_config_json.clone().unwrap_or_default();
        let is_raw_wire = wire_mode == 0;

        let host = open.host.trim().to_string();
        if is_raw_wire {
            if host.is_empty() {
                return Err(Status::invalid_argument("proxy host is empty"));
            }
            if open.port == 0 || open.port > 65535 {
                return Err(Status::invalid_argument("invalid proxy port"));
            }
            proxy_allowlist_check(&host)?;
        } else if wire_json.trim().is_empty() {
            return Err(Status::invalid_argument(
                "wire_config_json is required when wire_mode is not RAW_TCP_RELAY",
            ));
        }

        let wire_params_parsed: Option<wire_protocol::WireParams> = if is_raw_wire {
            None
        } else {
            Some(wire_protocol::WireParams::from_json(wire_json.trim()).map_err(|e| {
                Status::invalid_argument(format!("wire_config_json: {e}"))
            })?)
        };

        let client_pubkey_for_traffic = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let db_opt = self.db.clone();
        let metrics = self.proxy_metrics.clone();

        let require_token = std::env::var("DEPLOY_PROXY_REQUIRE_SESSION_TOKEN")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let token_s = open.session_token.trim();
        let managed: Option<(GrpcProxySessionRow, crate::proxy_session::StoredPolicy)> =
            if token_s.is_empty() {
                if require_token {
                    return Err(Status::failed_precondition(
                        "DEPLOY_PROXY_REQUIRE_SESSION_TOKEN requires session token; use CreateConnection",
                    ));
                }
                None
            } else {
                let db = self.db.as_ref().ok_or_else(|| {
                    Status::failed_precondition("metadata database required for session token")
                })?;
                let pk = client_pubkey_for_traffic.as_deref().ok_or_else(|| {
                    Status::unauthenticated("missing pubkey metadata for session token")
                })?;
                let hash = proxy_session::hash_session_token_hex(token_s);
                let row = db
                    .fetch_grpc_proxy_session_by_token_sha256(&hash)
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?;
                let row = row.ok_or_else(|| Status::permission_denied("invalid session token"))?;
                if row.revoked {
                    return Err(Status::permission_denied("session revoked"));
                }
                if row.client_pubkey_b64 != pk {
                    return Err(Status::permission_denied("session token not for this client key"));
                }
                let now = chrono::Utc::now();
                if row.expires_at < now {
                    return Err(Status::permission_denied("session expired"));
                }
                let policy = proxy_session::parse_policy_json(&row.policy_json)?;
                if !proxy_session::is_schedule_allowed_stored(&policy, now)? {
                    return Err(Status::permission_denied("outside access schedule"));
                }
                if proxy_session::active_time_budget_exceeded(
                    &policy,
                    row.active_ms.max(0) as u64,
                    0,
                ) {
                    return Err(Status::permission_denied(
                        "session active time budget exhausted",
                    ));
                }
                if proxy_session::max_duration_exceeded(&policy, row.first_open_at, now) {
                    return Err(Status::permission_denied("session max duration exceeded"));
                }
                proxy_session::check_traffic_limits(
                    &policy,
                    row.bytes_in.max(0) as u64,
                    row.bytes_out.max(0) as u64,
                )?;
                Some((row, policy))
            };

        let wire_params_parsed =
            apply_managed_wire_secrets(wire_params_parsed, wire_mode, &managed);

        let tunnel_id = Uuid::new_v4();
        let priority = open.tunnel_priority.unwrap_or(0);
        let stream_correlation_id = open.stream_correlation_id.clone();

        let admission_guard = self
            .tunnel_admission
            .clone()
            .acquire(
                priority,
                tunnel_id,
                self.tunnel_redis.as_deref(),
                tunnel_admission::wait_timeout_from_env(),
            )
            .await?;

        let checkpoint_interval = tunnel_admission::checkpoint_interval_from_env();
        let managed_checkpoint: Option<ManagedTunnelCheckpoint> = match (
            db_opt.as_ref(),
            managed.as_ref(),
        ) {
            (Some(db), Some((row, _))) => Some(ManagedTunnelCheckpoint {
                db: db.clone(),
                session_id: row.session_id.clone(),
                client_pubkey: row.client_pubkey_b64.clone(),
                counters: Arc::new(TokioMutex::new(TunnelFlushCounters::default())),
                interval: checkpoint_interval,
                redis: self.tunnel_redis.as_ref().map(|r| (r.clone(), tunnel_id)),
                tunnel_id,
                wire_mode: wire_mode as i32,
                priority,
                stream_correlation_id: stream_correlation_id.clone(),
            }),
            _ => None,
        };

        if let Some(ref red) = self.tunnel_redis {
            let session_max = managed
                .as_ref()
                .map(|(_, p)| p.effective_max_concurrent_devices_per_session())
                .unwrap_or(0);
            let fields = redis_fields_snapshot(
                &tunnel_id,
                managed.as_ref().map(|(r, _)| r.session_id.as_str()),
                client_pubkey_for_traffic.as_deref(),
                &stream_correlation_id,
                wire_mode as i32,
                priority,
                0,
                0,
                0,
                0,
            );
            let refs: Vec<(&str, &str)> = fields
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            match red.register_online(&tunnel_id, &refs, session_max).await {
                Ok(()) => {}
                Err(crate::tunnel_registry::RegisterOnlineError::SessionDeviceLimit) => {
                    return Err(Status::resource_exhausted(
                        "max concurrent proxy tunnels per session (device limit)",
                    ));
                }
                Err(crate::tunnel_registry::RegisterOnlineError::Redis(e)) => {
                    tracing::warn!(error = %e, "redis register_online");
                }
            }
        }

        let port = open.port;
        let addr = format!("{host}:{port}");
        let bytes_in = Arc::new(AtomicU64::new(0));
        let bytes_out = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel::<Result<ProxyServerMsg, Status>>(64);

        let managed_clone = managed.clone();
        let stream_corr = open.stream_correlation_id.clone();
        let tunnel_redis_cleanup = self.tunnel_redis.clone();
        let managed_checkpoint_wire = managed_checkpoint.clone();
        let managed_checkpoint_raw = managed_checkpoint.clone();
        let prefer_quic_open = open.prefer_quic_data_plane.unwrap_or(false);
        let quic_dp = self.quic_dataplane.clone();
        tokio::spawn(async move {
            let _redis_drop = RedisTunnelDropGuard {
                redis: tunnel_redis_cleanup.clone(),
                id: tunnel_id,
            };
            metrics.tunnels_total.fetch_add(1, Ordering::Relaxed);
            metrics.tunnels_open.fetch_add(1, Ordering::Relaxed);
            struct OpenGuard<'a>(&'a Arc<ProxyTunnelMetrics>);
            impl Drop for OpenGuard<'_> {
                fn drop(&mut self) {
                    self.0.tunnels_open.fetch_sub(1, Ordering::Relaxed);
                }
            }
            let _open_guard = OpenGuard(&metrics);

            if !stream_corr.is_empty() {
                tracing::info!(stream_correlation_id = %stream_corr, "ProxyTunnel");
            }

            if let Some(params) = wire_params_parsed {
                let _admission_guard = admission_guard;
                match wire_mode {
                    1 => {
                        crate::wire_relay::run_vless_relay(
                            inbound,
                            tx,
                            params,
                            managed_clone,
                            bytes_in,
                            bytes_out,
                            db_opt,
                            client_pubkey_for_traffic,
                            metrics.clone(),
                            managed_checkpoint_wire.clone(),
                        )
                        .await;
                    }
                    2 => {
                        crate::wire_relay::run_trojan_relay(
                            inbound,
                            tx,
                            params,
                            managed_clone,
                            bytes_in,
                            bytes_out,
                            db_opt,
                            client_pubkey_for_traffic,
                            metrics.clone(),
                            managed_checkpoint_wire.clone(),
                        )
                        .await;
                    }
                    3 => {
                        crate::wire_relay::run_vmess_relay(
                            inbound,
                            tx,
                            params,
                            managed_clone,
                            bytes_in,
                            bytes_out,
                            db_opt,
                            client_pubkey_for_traffic,
                            metrics.clone(),
                            managed_checkpoint_wire.clone(),
                        )
                        .await;
                    }
                    4 => {
                        crate::wire_relay::run_shadowsocks_relay(
                            inbound,
                            tx,
                            params,
                            managed_clone,
                            bytes_in,
                            bytes_out,
                            db_opt,
                            client_pubkey_for_traffic,
                            metrics.clone(),
                            managed_checkpoint_wire.clone(),
                        )
                        .await;
                    }
                    5 => {
                        crate::wire_relay::run_socks5_relay(
                            inbound,
                            tx,
                            params,
                            managed_clone,
                            bytes_in,
                            bytes_out,
                            db_opt,
                            client_pubkey_for_traffic,
                            metrics.clone(),
                            managed_checkpoint_wire.clone(),
                        )
                        .await;
                    }
                    _ => {
                        metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                        let _ = tx
                            .send(Ok(ProxyServerMsg {
                                body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                                    ok: false,
                                    error: "unknown wire_mode".into(),
                                    quic_data_plane: false,
                                    quic_host: String::new(),
                                    quic_port: 0,
                                    data_plane_ticket: Vec::new(),
                                })),
                            }))
                            .await;
                    }
                }
                return;
            }

            let use_quic = crate::quic::dataplane_enabled()
                && quic_dp.is_some()
                && prefer_quic_open;
            if use_quic {
                let Some(qdp) = quic_dp else {
                    return;
                };
                let mut ticket_bin = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut ticket_bin);
                let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
                let idle_secs = managed_clone
                    .as_ref()
                    .map(|(_, p)| p.idle_timeout_sec())
                    .unwrap_or(60);
                let active = Arc::new(StdMutex::new(crate::quic::context::ActiveTracker::new(
                    idle_secs,
                )));
                let session_id_for_task_q = managed_clone
                    .as_ref()
                    .map(|(r, _)| r.session_id.clone());
                let pk_for_task_q = managed_clone
                    .as_ref()
                    .map(|(r, _)| r.client_pubkey_b64.clone());
                let policy_for_task_q = managed_clone.as_ref().map(|(_, p)| p.clone());
                let base_in_q = managed_clone
                    .as_ref()
                    .map(|(r, _)| r.bytes_in.max(0) as u64)
                    .unwrap_or(0);
                let base_out_q = managed_clone
                    .as_ref()
                    .map(|(r, _)| r.bytes_out.max(0) as u64)
                    .unwrap_or(0);
                let base_active_ms_q = managed_clone
                    .as_ref()
                    .map(|(r, _)| r.active_ms.max(0) as u64)
                    .unwrap_or(0);

                let ctx = crate::quic::context::QuicRawContext {
                    expected_host: host.clone(),
                    expected_port: port as u16,
                    tunnel_id,
                    admission_guard,
                    managed_checkpoint: managed_checkpoint_raw.clone(),
                    bytes_in: bytes_in.clone(),
                    bytes_out: bytes_out.clone(),
                    metrics: metrics.clone(),
                    db_opt: db_opt.clone(),
                    client_pubkey_for_traffic: client_pubkey_for_traffic.clone(),
                    stream_correlation_id: stream_corr.clone(),
                    session_id_for_task: session_id_for_task_q,
                    pk_for_task: pk_for_task_q,
                    policy_for_task: policy_for_task_q,
                    base_in: base_in_q,
                    base_out: base_out_q,
                    base_active_ms: base_active_ms_q,
                    active,
                    completion: completion_tx,
                };
                qdp.ticket_store.insert(ticket_bin, ctx).await;
                let qh = qdp.public_host.clone();
                let qp = if qdp.public_port > 0 {
                    qdp.public_port
                } else {
                    7844
                };
                if tx
                    .send(Ok(ProxyServerMsg {
                        body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                            ok: true,
                            error: String::new(),
                            quic_data_plane: true,
                            quic_host: qh,
                            quic_port: qp as u32,
                            data_plane_ticket: ticket_bin.to_vec(),
                        })),
                    }))
                    .await
                    .is_err()
                {
                    return;
                }

                let mut inbound_drain = inbound;
                let drain_jh = tokio::spawn(async move {
                    while let Some(item) = inbound_drain.next().await {
                        let msg = match item {
                            Ok(m) => m,
                            Err(_) => break,
                        };
                        match msg.body {
                            Some(proxy_client_msg::Body::Open(_)) => break,
                            Some(proxy_client_msg::Body::Fin(_)) | None => break,
                            Some(proxy_client_msg::Body::Data(_)) => {}
                        }
                    }
                });

                match tokio::time::timeout(Duration::from_secs(120), completion_rx).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => {
                        metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                drain_jh.abort();
                let _ = drain_jh.await;
                return;
            }

            let _admission_guard = admission_guard;

            let tcp = match tokio::time::timeout(
                Duration::from_secs(30),
                tokio::net::TcpStream::connect(&addr),
            )
            .await
            {
                Err(_) => {
                    metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                                ok: false,
                                error: "connect timeout".to_string(),
                                quic_data_plane: false,
                                quic_host: String::new(),
                                quic_port: 0,
                                data_plane_ticket: Vec::new(),
                            })),
                        }))
                        .await;
                    return;
                }
                Ok(Err(e)) => {
                    metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                                ok: false,
                                error: e.to_string(),
                                quic_data_plane: false,
                                quic_host: String::new(),
                                quic_port: 0,
                                data_plane_ticket: Vec::new(),
                            })),
                        }))
                        .await;
                    return;
                }
                Ok(Ok(s)) => s,
            };

            if tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                        ok: true,
                        error: String::new(),
                        quic_data_plane: false,
                        quic_host: String::new(),
                        quic_port: 0,
                        data_plane_ticket: Vec::new(),
                    })),
                }))
                .await
                .is_err()
            {
                return;
            }

            const MAX_PROXY_CHUNK: usize = 256 * 1024;
            let (mut tcp_read, mut tcp_write) = tcp.into_split();

            struct ActiveTracker {
                last: Option<Instant>,
                idle: Duration,
                accum_ms: u64,
            }
            impl ActiveTracker {
                fn new(idle_secs: u64) -> Self {
                    Self {
                        last: None,
                        idle: Duration::from_secs(idle_secs.max(1)),
                        accum_ms: 0,
                    }
                }
                fn bump(&mut self) {
                    let now = Instant::now();
                    if let Some(prev) = self.last {
                        let g = now.saturating_duration_since(prev);
                        if g <= self.idle {
                            self.accum_ms += g.as_millis() as u64;
                        }
                    }
                    self.last = Some(now);
                }
            }

            let idle_secs = managed_clone
                .as_ref()
                .map(|(_, p)| p.idle_timeout_sec())
                .unwrap_or(60);
            let active = Arc::new(StdMutex::new(ActiveTracker::new(idle_secs)));
            let active_in = active.clone();
            let active_out = active.clone();
            let active_end = active;

            let mut checkpoint_jh: Option<tokio::task::JoinHandle<()>> = None;
            let mut checkpoint_shut: Option<tokio::sync::watch::Sender<bool>> = None;
            if let Some(ref cp) = managed_checkpoint_raw {
                let (jh, tx) = spawn_managed_checkpoint(
                    cp.clone(),
                    bytes_in.clone(),
                    bytes_out.clone(),
                    {
                        let active_c = active_end.clone();
                        move || active_c.lock().map(|a| a.accum_ms).unwrap_or(0)
                    },
                );
                checkpoint_jh = Some(jh);
                checkpoint_shut = Some(tx);
            }

            let session_id_for_task = managed_clone
                .as_ref()
                .map(|(r, _)| r.session_id.clone());
            let pk_for_task = managed_clone
                .as_ref()
                .map(|(r, _)| r.client_pubkey_b64.clone());
            let policy_for_task = managed_clone.as_ref().map(|(_, p)| p.clone());
            let policy_for_t_in = policy_for_task.clone();
            let base_in = managed_clone
                .as_ref()
                .map(|(r, _)| r.bytes_in.max(0) as u64)
                .unwrap_or(0);
            let base_out = managed_clone
                .as_ref()
                .map(|(r, _)| r.bytes_out.max(0) as u64)
                .unwrap_or(0);
            let base_active_ms = managed_clone
                .as_ref()
                .map(|(r, _)| r.active_ms.max(0) as u64)
                .unwrap_or(0);

            let bin_count = bytes_in.clone();
            let bytes_out_in = bytes_out.clone();
            let mut inbound = inbound;
            let t_in = tokio::spawn(async move {
                while let Some(item) = inbound.next().await {
                    let msg = match item {
                        Ok(m) => m,
                        Err(e) => {
                            let _ = tcp_write.shutdown().await;
                            return Err(Status::internal(e.to_string()));
                        }
                    };
                    match msg.body {
                        Some(proxy_client_msg::Body::Open(_)) => {
                            let _ = tcp_write.shutdown().await;
                            return Err(Status::invalid_argument("duplicate Open"));
                        }
                        Some(proxy_client_msg::Body::Data(data)) => {
                            if data.len() > MAX_PROXY_CHUNK {
                                let _ = tcp_write.shutdown().await;
                                return Err(Status::invalid_argument("proxy chunk too large"));
                            }
                            bin_count.fetch_add(data.len() as u64, Ordering::Relaxed);
                            let mut traffic_exceeded = false;
                            let mut budget_exceeded = false;
                            if let Ok(mut a) = active_in.lock() {
                                a.bump();
                                if let Some(ref pol) = policy_for_t_in {
                                    let bi = base_in + bin_count.load(Ordering::Relaxed);
                                    let bo = base_out + bytes_out_in.load(Ordering::Relaxed);
                                    traffic_exceeded =
                                        proxy_session::check_traffic_limits(pol, bi, bo).is_err();
                                    if !traffic_exceeded {
                                        budget_exceeded = proxy_session::active_time_budget_exceeded(
                                            pol,
                                            base_active_ms,
                                            a.accum_ms,
                                        );
                                    }
                                }
                            }
                            if traffic_exceeded {
                                let _ = tcp_write.shutdown().await;
                                return Err(Status::resource_exhausted(
                                    "proxy session traffic limit",
                                ));
                            }
                            if budget_exceeded {
                                let _ = tcp_write.shutdown().await;
                                return Err(Status::resource_exhausted(
                                    "proxy session active time budget exhausted",
                                ));
                            }
                            if let Err(e) = tcp_write.write_all(&data).await {
                                let _ = tcp_write.shutdown().await;
                                return Err(Status::internal(e.to_string()));
                            }
                        }
                        Some(proxy_client_msg::Body::Fin(_)) => {
                            let _ = tcp_write.shutdown().await;
                            return Ok(());
                        }
                        None => {}
                    }
                }
                let _ = tcp_write.shutdown().await;
                Ok(())
            });

            let mut buf = vec![0u8; MAX_PROXY_CHUNK];
            let tx_out = tx.clone();
            let bout_count = bytes_out.clone();
            let bytes_in_out = bytes_in.clone();
            let policy_out = policy_for_task.clone();
            let base_in_out = base_in;
            let base_out_out = base_out;
            let t_out = tokio::spawn(async move {
                loop {
                    match tcp_read.read(&mut buf).await {
                        Ok(0) => {
                            let _ = tx_out
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::Eof(true)),
                                }))
                                .await;
                            break;
                        }
                        Ok(n) => {
                            bout_count.fetch_add(n as u64, Ordering::Relaxed);
                            let mut budget_exceeded = false;
                            if let Ok(mut a) = active_out.lock() {
                                a.bump();
                                if let Some(ref pol) = policy_out {
                                    let bi = base_in_out + bytes_in_out.load(Ordering::Relaxed);
                                    let bo = base_out_out + bout_count.load(Ordering::Relaxed);
                                    let _ = proxy_session::check_traffic_limits(pol, bi, bo);
                                    budget_exceeded = proxy_session::active_time_budget_exceeded(
                                        pol,
                                        base_active_ms,
                                        a.accum_ms,
                                    );
                                }
                            }
                            if budget_exceeded {
                                let _ = tx_out
                                    .send(Ok(ProxyServerMsg {
                                        body: Some(proxy_server_msg::Body::Error(
                                            "proxy session active time budget exhausted".into(),
                                        )),
                                    }))
                                    .await;
                                break;
                            }
                            let chunk = buf[..n].to_vec();
                            if tx_out
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::Data(chunk)),
                                }))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx_out
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::Error(e.to_string())),
                                }))
                                .await;
                            break;
                        }
                    }
                }
            });

            let (r_in, _r_out) = tokio::join!(t_in, t_out);
            match r_in {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                    let _ = tx.send(Err(e)).await;
                }
                Err(e) => {
                    metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
                    let _ = tx
                        .send(Err(Status::internal(format!("proxy client task: {e}"))))
                        .await;
                }
            }

            if let Some(tx) = checkpoint_shut {
                let _ = tx.send(true);
            }
            if let Some(jh) = checkpoint_jh {
                jh.abort();
            }

            let bi = bytes_in.load(Ordering::Relaxed);
            let bo = bytes_out.load(Ordering::Relaxed);
            let active_ms_u64 = active_end
                .lock()
                .map(|a| a.accum_ms)
                .unwrap_or(0);

            if let Some(ref cp) = managed_checkpoint_raw {
                if let Err(e) = flush_managed_tunnel_end(cp, bi, bo, active_ms_u64).await {
                    error!(%e, "grpc proxy session final flush (raw tunnel)");
                }
            } else if let (Some(db), Some(sid), Some(pk), Some(_pol)) = (
                db_opt.clone(),
                session_id_for_task,
                pk_for_task,
                policy_for_task,
            ) {
                let now = chrono::Utc::now();
                let _ = db
                    .increment_grpc_proxy_session_traffic(
                        &sid,
                        &pk,
                        bi,
                        bo,
                        active_ms_u64 as i64,
                        now,
                        Some(now),
                    )
                    .await;
            }

            metrics.bytes_in.fetch_add(bi, Ordering::Relaxed);
            metrics.bytes_out.fetch_add(bo, Ordering::Relaxed);

            let db_for_hourly = db_opt.clone();
            if let (Some(db), Some(pk)) = (db_for_hourly, client_pubkey_for_traffic.clone()) {
                if bi > 0 || bo > 0 {
                    let hour = floor_to_utc_hour(chrono::Utc::now());
                    let db2 = db.clone();
                    tokio::spawn(async move {
                        if let Err(e) = db2.add_grpc_proxy_traffic_hourly(&pk, hour, bi, bo).await {
                            error!(%e, "grpc proxy traffic hourly");
                        }
                    });
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))
            as Pin<Box<dyn Stream<Item = Result<ProxyServerMsg, Status>> + Send + 'static>>))
    }

    async fn update_connection_profile(
        &self,
        request: Request<UpdateConnectionProfileRequest>,
    ) -> Result<Response<UpdateConnectionProfileResponse>, Status> {
        const NO_METADATA_DB: &str = "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner_ref = request.get_ref();
        validate_project_id(&inner_ref.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("UpdateConnectionProfile", &inner_ref.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "UpdateConnectionProfile",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing client pubkey"))?;
        let inner = request.into_inner();
        db.upsert_grpc_peer_profile(
            pk,
            inner.connection_kind as i16,
            inner.agent_version.trim(),
        )
        .await
        .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(UpdateConnectionProfileResponse {
            status: "ok".to_string(),
        }))
    }

    async fn report_resource_usage(
        &self,
        request: Request<ReportResourceUsageRequest>,
    ) -> Result<Response<ReportResourceUsageResponse>, Status> {
        const NO_METADATA_DB: &str = "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner_ref = request.get_ref();
        validate_project_id(&inner_ref.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("ReportResourceUsage", &inner_ref.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "ReportResourceUsage",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing client pubkey"))?;
        let inner = request.into_inner();
        db.upsert_grpc_peer_resource_snapshot(
            pk,
            inner.cpu_percent,
            inner.ram_percent,
            inner.gpu_percent,
            inner.ram_used_bytes.map(|v| v as i64),
            inner.storage_used_bytes.map(|v| v as i64),
        )
        .await
        .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(ReportResourceUsageResponse {
            status: "ok".to_string(),
        }))
    }

    async fn report_display_topology(
        &self,
        request: Request<ReportDisplayTopologyRequest>,
    ) -> Result<Response<ReportDisplayTopologyResponse>, Status> {
        const NO_METADATA_DB: &str = "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner_ref = request.get_ref();
        validate_project_id(&inner_ref.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("ReportDisplayTopology", &inner_ref.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "ReportDisplayTopology",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing client pubkey"))?;
        let inner = request.into_inner();
        let v: Vec<serde_json::Value> = inner
            .displays
            .iter()
            .map(|d| {
                serde_json::json!({
                    "index": d.index,
                    "label": d.label,
                    "width": d.width,
                    "height": d.height,
                })
            })
            .collect();
        let json = serde_json::to_string(&v).map_err(|e| Status::internal(e.to_string()))?;
        db.upsert_peer_display_topology(pk, inner.stream_capable, &json)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(ReportDisplayTopologyResponse {
            status: "ok".to_string(),
        }))
    }

    async fn connection_probe(
        &self,
        request: Request<Streaming<ConnectionProbeChunk>>,
    ) -> Result<Response<ConnectionProbeResult>, Status> {
        let extensions = request.extensions().clone();
        let meta = request.metadata().clone();
        let mut stream = request.into_inner();

        let first = stream
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("empty stream"))?
            .map_err(|e| Status::internal(e.to_string()))?;

        validate_project_id(&first.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("ConnectionProbe", &first.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "ConnectionProbe",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            let mut req_stub = Request::new(());
            *req_stub.extensions_mut() = extensions;
            register_authenticated_client(&req_stub, &meta);
        }

        let project_norm = normalize_project_id(&first.project_id);
        let mut download_req = first.download_request_bytes as usize;
        if download_req == 0 {
            download_req = CONNECTION_PROBE_DEFAULT_DOWNLOAD_BYTES;
        }
        download_req = download_req.min(CONNECTION_PROBE_MAX_DOWNLOAD_BYTES);

        let upload_start = std::time::Instant::now();
        let mut total: u64 = first.data.len() as u64;
        if total > CONNECTION_PROBE_MAX_UPLOAD_BYTES {
            return Err(Status::invalid_argument("upload exceeds cap"));
        }

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| Status::internal(e.to_string()))?;
            if !chunk.project_id.is_empty()
                && normalize_project_id(&chunk.project_id) != project_norm
            {
                return Err(Status::invalid_argument(
                    "project_id must only be set on first chunk",
                ));
            }
            let n = chunk.data.len() as u64;
            total = total
                .checked_add(n)
                .ok_or_else(|| Status::invalid_argument("upload size overflow"))?;
            if total > CONNECTION_PROBE_MAX_UPLOAD_BYTES {
                return Err(Status::invalid_argument("upload exceeds cap"));
            }
        }

        let upload_duration_ms = upload_start.elapsed().as_millis() as i64;

        let mut download_payload = vec![0u8; download_req];
        for (i, b) in download_payload.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(0x5a);
        }

        Ok(Response::new(ConnectionProbeResult {
            upload_bytes: total,
            upload_duration_ms,
            download_payload,
        }))
    }

    async fn create_connection(
        &self,
        request: Request<CreateConnectionRequest>,
    ) -> Result<Response<CreateConnectionResponse>, Status> {
        const NO_METADATA_DB: &str =
            "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner = request.get_ref();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("CreateConnection", &inner.project_id, "");
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; CreateConnection unavailable")
        })?;
        {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "CreateConnection",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }

        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing pubkey"))?;

        let pk_stored = if let Some(ref r) = inner.recipient_client_pubkey_b64 {
            let t = r.trim();
            if t.is_empty() {
                pk.to_string()
            } else {
                let vk = parse_verifying_key_b64(t)
                    .map_err(|e| Status::invalid_argument(format!("invalid recipient pubkey: {e}")))?;
                raw_pubkey_b64_url(&vk.to_bytes())
            }
        } else {
            pk.to_string()
        };

        let policy = inner.policy.clone().unwrap_or_default();
        let now = chrono::Utc::now();
        if !proxy_session::is_schedule_allowed(&policy, now)? {
            return Err(Status::permission_denied("outside access schedule"));
        }
        let policy_json = proxy_session::policy_json_from_proto(&policy)?;
        let expires_at = proxy_session::expires_at_from_policy(&policy, now);

        let (wire_mode, wire_json_owned) = validate_inbound_wire(
            inner.wire_mode,
            inner.wire_config_json.as_deref(),
        )?;
        let wire_json_ref = wire_json_owned.as_deref();

        let ingress = validate_ingress_create(inner)?;

        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let token = hex::encode(raw);
        let token_hash = proxy_session::hash_session_token_hex(&token);

        let mut sub_raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut sub_raw);
        let subscription_token = hex::encode(sub_raw);

        let session_id = db
            .insert_grpc_proxy_session(
                pk_stored.as_str(),
                inner.board_label.as_str(),
                &token_hash,
                subscription_token.as_str(),
                expires_at,
                &policy_json,
                wire_mode,
                wire_json_ref,
                ingress.protocol,
                ingress.listen_tcp,
                ingress.listen_udp,
                ingress.config_json.as_deref(),
                ingress.tls_json.as_deref(),
                ingress.template_version,
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let expires_ms = proxy_session::client_expires_at_unix_ms(&policy, expires_at);

        let subscription_url = std::env::var("DEPLOY_SUBSCRIPTION_PUBLIC_HOST")
            .ok()
            .map(|b| b.trim().trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .map(|base| {
                format!("{base}/api/v1/public/proxy-subscription/{subscription_token}")
            });

        Ok(Response::new(CreateConnectionResponse {
            session_id,
            session_token: token,
            expires_at_unix_ms: expires_ms,
            status: "ok".to_string(),
            subscription_token: Some(subscription_token),
            subscription_url,
        }))
    }

    async fn close_connection(
        &self,
        request: Request<CloseConnectionRequest>,
    ) -> Result<Response<CloseConnectionResponse>, Status> {
        const NO_METADATA_DB: &str =
            "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner = request.get_ref();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("CloseConnection", &inner.project_id, "");
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; CloseConnection unavailable")
        })?;
        {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "CloseConnection",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }

        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing pubkey"))?;

        let n = db
            .revoke_grpc_proxy_session(&inner.session_id, pk)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        if n == 0 {
            return Err(Status::not_found("session not found or already revoked"));
        }
        Ok(Response::new(CloseConnectionResponse {
            status: "ok".to_string(),
        }))
    }

    async fn get_stats(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<GetStatsResponse>, Status> {
        const NO_METADATA_DB: &str =
            "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner = request.get_ref();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("GetStats", &inner.project_id, "");
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; GetStats unavailable")
        })?;
        {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetStats",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }

        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing pubkey"))?;

        if inner.session_id.is_empty() {
            let agg = db
                .aggregate_grpc_proxy_sessions_for_pubkey(pk)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            return Ok(Response::new(GetStatsResponse {
                session_id: String::new(),
                bytes_in: agg.bytes_in,
                bytes_out: agg.bytes_out,
                active_ms: agg.active_ms,
                last_activity_unix_ms: agg
                    .last_activity_at
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0),
                revoked: false,
                created_at_unix_ms: agg
                    .created_at_max
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0),
                expires_at_unix_ms: agg
                    .expires_at_min
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0),
            }));
        }

        let row = db
            .fetch_grpc_proxy_session_by_id(&inner.session_id, pk)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let row = row.ok_or_else(|| Status::not_found("session not found"))?;
        Ok(Response::new(GetStatsResponse {
            session_id: row.session_id,
            bytes_in: row.bytes_in.max(0) as u64,
            bytes_out: row.bytes_out.max(0) as u64,
            active_ms: row.active_ms.max(0) as u64,
            last_activity_unix_ms: row
                .last_activity_at
                .map(|t| t.timestamp_millis())
                .unwrap_or(0),
            revoked: row.revoked,
            created_at_unix_ms: row.created_at.timestamp_millis(),
            expires_at_unix_ms: expires_ms_from_policy_json(&row.policy_json, row.expires_at),
        }))
    }

    async fn update_settings(
        &self,
        request: Request<UpdateProxySettingsRequest>,
    ) -> Result<Response<UpdateProxySettingsResponse>, Status> {
        const NO_METADATA_DB: &str =
            "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        let inner = request.get_ref();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("UpdateSettings", &inner.project_id, "");
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; UpdateSettings unavailable")
        })?;
        {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "UpdateSettings",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }

        let pk = meta
            .get(META_PUBKEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing pubkey"))?;

        let policy = inner.policy.clone().unwrap_or_default();
        let now = chrono::Utc::now();
        if !proxy_session::is_schedule_allowed(&policy, now)? {
            return Err(Status::permission_denied("outside access schedule"));
        }
        let policy_json = proxy_session::policy_json_from_proto(&policy)?;
        let expires_at = proxy_session::expires_at_from_policy(&policy, now);

        let update_wire = inner.wire_mode.is_some() || inner.wire_config_json.is_some();
        let (wire_mode, wire_json_owned) = if update_wire {
            validate_inbound_wire(inner.wire_mode, inner.wire_config_json.as_deref())?
        } else {
            (None, None)
        };
        let wire_json_ref = wire_json_owned.as_deref();

        let update_ingress = should_update_ingress(inner);
        let ingress = if update_ingress {
            validate_ingress_update(inner)?
        } else {
            IngressDb::disabled()
        };

        let n = db
            .update_grpc_proxy_session_policy(
                &inner.session_id,
                pk,
                &policy_json,
                expires_at,
                update_wire,
                wire_mode,
                wire_json_ref,
                update_ingress,
                ingress.protocol,
                ingress.listen_tcp,
                ingress.listen_udp,
                ingress.config_json.as_deref(),
                ingress.tls_json.as_deref(),
                ingress.template_version,
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        if n == 0 {
            return Err(Status::not_found("session not found or revoked"));
        }
        let expires_ms = proxy_session::client_expires_at_unix_ms(&policy, expires_at);
        Ok(Response::new(UpdateProxySettingsResponse {
            status: "ok".to_string(),
            expires_at_unix_ms: expires_ms,
        }))
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        const NO_METADATA_DB: &str = "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("ListSessions", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "ListSessions",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let _ = request.into_inner();

        let agg = db
            .fetch_grpc_session_peer_last_activity()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let mut agg_map: HashMap<String, deploy_db::GrpcSessionPeerAggregateRow> =
            agg.into_iter()
                .map(|r| (r.client_pubkey_b64.clone(), r))
                .collect();

        let dbr: &DbStore = db.as_ref();
        let mut peers = Vec::new();
        if let Some(ref auth) = self.auth {
            let mut keys: Vec<String> = {
                let peer_set = auth.peers.read();
                peer_set.iter().map(raw_pubkey_b64_url).collect()
            };
            keys.sort();
            for pk in keys {
                let row = agg_map.remove(&pk);
                let (last_seen_ms, last_peer_ip, last_grpc_method) = match row {
                    Some(r) => (
                        r.last_created_at.timestamp_millis(),
                        r.last_peer_ip,
                        r.last_grpc_method,
                    ),
                    None => (0_i64, String::new(), String::new()),
                };
                peers.push(
                    session_peer_row_enriched(
                        dbr,
                        pk,
                        last_seen_ms,
                        last_peer_ip,
                        last_grpc_method,
                    )
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?,
                );
            }
        } else {
            let mut rest: Vec<_> = agg_map.into_values().collect();
            rest.sort_by(|a, b| a.client_pubkey_b64.cmp(&b.client_pubkey_b64));
            for r in rest {
                peers.push(
                    session_peer_row_enriched(
                        dbr,
                        r.client_pubkey_b64,
                        r.last_created_at.timestamp_millis(),
                        r.last_peer_ip,
                        r.last_grpc_method,
                    )
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?,
                );
            }
        }

        Ok(Response::new(ListSessionsResponse { peers }))
    }

    async fn query_session_logs(
        &self,
        request: Request<QuerySessionLogsRequest>,
    ) -> Result<Response<QuerySessionLogsResponse>, Status> {
        const NO_METADATA_DB: &str = "metadata database is not configured; set DEPLOY_SQLITE_URL or DATABASE_URL on deploy-server";
        let Some(db) = self.db.as_ref() else {
            return Err(Status::failed_precondition(NO_METADATA_DB));
        };
        let meta = request.metadata().clone();
        validate_project_id(&request.get_ref().project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("QuerySessionLogs", &request.get_ref().project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "QuerySessionLogs",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
            register_authenticated_client(&request, &meta);
        }
        let inner = request.into_inner();
        let page = if inner.limit <= 0 {
            50
        } else {
            inner.limit.min(500)
        } as i64;
        let fetch_n = page + 1;
        let mut rows = db
            .fetch_grpc_session_events_page(inner.before_id, fetch_n)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let has_more = rows.len() as i64 > page;
        rows.truncate(page as usize);
        let events = rows
            .into_iter()
            .map(|r| SessionLogEvent {
                id: r.id,
                created_at_ms: r.created_at.timestamp_millis(),
                kind: r.kind,
                client_public_key_b64: r.client_pubkey_b64.unwrap_or_default(),
                peer_ip: r.peer_ip,
                grpc_method: r.grpc_method,
                status: r.status,
                detail: r.detail,
            })
            .collect();
        Ok(Response::new(QuerySessionLogsResponse { events, has_more }))
    }
}

fn proxy_allowlist_check(host: &str) -> Result<(), Status> {
    let Ok(list) = std::env::var("DEPLOY_PROXY_ALLOWLIST") else {
        return Ok(());
    };
    let list = list.trim();
    if list.is_empty() || list == "*" {
        return Ok(());
    }
    let host_lc = host.to_ascii_lowercase();
    for part in list.split(',') {
        let p = part.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if host_lc == p || host_lc.ends_with(&format!(".{p}")) {
            return Ok(());
        }
    }
    Err(Status::permission_denied(
        "proxy target host not allowed by DEPLOY_PROXY_ALLOWLIST",
    ))
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
        cmd.status().await.map_err(|e| format!("sudo apply stack: {e}"))
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
        cmd.status().await.map_err(|e| format!("powershell apply stack: {e}"))
    }
}

fn find_pirate_bundle_root(extracted: &Path) -> Result<PathBuf, Status> {
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
    Err(Status::invalid_argument(
        "expected bundle with bin/deploy-server and bin/control-api (top-level or pirate-*-*/)",
    ))
}

fn parse_stack_manifest_versions(s: &str) -> (Option<String>, Option<String>) {
    let Some(v) = serde_json::from_str::<serde_json::Value>(s).ok() else {
        return (None, None);
    };
    (
        v.get("deploy_server")
            .and_then(|x| x.as_str())
            .map(String::from),
        v.get("control_api").and_then(|x| x.as_str()).map(String::from),
    )
}

fn host_mount_to_proto(m: &HostMountStats) -> HostMountStatsProto {
    HostMountStatsProto {
        path: m.path.clone(),
        total_bytes: m.total_bytes,
        free_bytes: m.free_bytes,
    }
}

fn host_net_to_proto(n: &HostNetInterface) -> HostNetInterfaceProto {
    HostNetInterfaceProto {
        name: n.name.clone(),
        rx_bytes_per_s: n.rx_bytes_per_s,
        tx_bytes_per_s: n.tx_bytes_per_s,
        rx_errors: n.rx_errors,
        tx_errors: n.tx_errors,
    }
}

fn host_log_line_to_proto(l: &HostLogLine) -> HostLogLineProto {
    HostLogLineProto {
        ts_ms: l.ts_ms,
        level: l.level.clone(),
        message: l.message.clone(),
    }
}

fn host_stats_view_to_proto(v: &HostStatsView) -> HostStatsResponse {
    HostStatsResponse {
        disk_free_bytes: v.disk_free_bytes,
        disk_total_bytes: v.disk_total_bytes,
        disk_mount_path: v.disk_mount_path.clone(),
        memory_used_bytes: v.memory_used_bytes,
        memory_total_bytes: v.memory_total_bytes,
        cpu_usage_percent: v.cpu_usage_percent,
        load_average_1m: v.load_average_1m,
        load_average_5m: v.load_average_5m,
        load_average_15m: v.load_average_15m,
        temperature_current_celsius: v.temperature_current_celsius,
        temperature_avg_celsius: v.temperature_avg_celsius,
        process_count: v.process_count as u64,
        disk_mounts: v.disk_mounts.iter().map(host_mount_to_proto).collect(),
        network_interfaces: v
            .network_interfaces
            .iter()
            .map(host_net_to_proto)
            .collect(),
        log_tail: v.log_tail.iter().map(host_log_line_to_proto).collect(),
    }
}

fn load_avg_to_proto(l: &LoadAvg) -> LoadAvgProto {
    LoadAvgProto {
        m1: l.m1,
        m5: l.m5,
        m15: l.m15,
    }
}

fn cpu_times_to_proto(t: &CpuTimes) -> CpuTimesProto {
    CpuTimesProto {
        user_ms: t.user_ms,
        system_ms: t.system_ms,
        idle_ms: t.idle_ms,
    }
}

fn process_cpu_to_proto(p: &ProcessCpu) -> ProcessCpuProto {
    ProcessCpuProto {
        pid: p.pid,
        name: p.name.clone(),
        cpu_percent: p.cpu_percent,
    }
}

fn series_hint_to_proto(s: &SeriesHint) -> SeriesHintProto {
    SeriesHintProto {
        available_ranges: s.available_ranges.clone(),
    }
}

fn cpu_detail_to_proto(d: &CpuDetail) -> CpuDetailProto {
    CpuDetailProto {
        ts_ms: d.ts_ms,
        loadavg: Some(load_avg_to_proto(&d.loadavg)),
        times: d.times.as_ref().map(cpu_times_to_proto),
        top_processes: d.top_processes.iter().map(process_cpu_to_proto).collect(),
        series_hint: Some(series_hint_to_proto(&d.series_hint)),
    }
}

fn memory_overview_to_proto(m: &MemoryOverview) -> MemoryOverviewProto {
    MemoryOverviewProto {
        total_bytes: m.total_bytes,
        used_bytes: m.used_bytes,
        available_bytes: m.available_bytes,
        cached_bytes: m.cached_bytes,
        buffers_bytes: m.buffers_bytes,
        swap_total_bytes: m.swap_total_bytes,
        swap_used_bytes: m.swap_used_bytes,
    }
}

fn process_mem_to_proto(p: &ProcessMem) -> ProcessMemProto {
    ProcessMemProto {
        pid: p.pid,
        name: p.name.clone(),
        memory_bytes: p.memory_bytes,
    }
}

fn memory_detail_to_proto(d: &MemoryDetail) -> MemoryDetailProto {
    MemoryDetailProto {
        ts_ms: d.ts_ms,
        memory: Some(memory_overview_to_proto(&d.memory)),
        top_processes: d.top_processes.iter().map(process_mem_to_proto).collect(),
    }
}

fn disk_io_to_proto(d: &DiskIoSummary) -> DiskIoSummaryProto {
    DiskIoSummaryProto {
        note: d.note.to_string(),
    }
}

fn process_disk_to_proto(p: &ProcessDisk) -> ProcessDiskProto {
    ProcessDiskProto {
        pid: p.pid,
        name: p.name.clone(),
        read_bytes: p.read_bytes,
        write_bytes: p.write_bytes,
    }
}

fn disk_detail_to_proto(d: &DiskDetail) -> DiskDetailProto {
    DiskDetailProto {
        ts_ms: d.ts_ms,
        mounts: d.mounts.iter().map(host_mount_to_proto).collect(),
        io: d.io.as_ref().map(disk_io_to_proto),
        top_processes: d.top_processes.iter().map(process_disk_to_proto).collect(),
    }
}

fn network_detail_to_proto(d: &NetworkDetail) -> NetworkDetailProto {
    NetworkDetailProto {
        ts_ms: d.ts_ms,
        interfaces: d.interfaces.iter().map(host_net_to_proto).collect(),
        connections_note: d.connections_note.to_string(),
    }
}

fn process_row_to_proto(p: &ProcessRow) -> ProcessRowProto {
    ProcessRowProto {
        pid: p.pid,
        name: p.name.clone(),
        cpu_percent: p.cpu_percent,
        memory_bytes: p.memory_bytes,
    }
}

fn processes_detail_to_proto(d: &ProcessesDetail) -> ProcessesDetailProto {
    ProcessesDetailProto {
        ts_ms: d.ts_ms,
        processes: d.processes.iter().map(process_row_to_proto).collect(),
        total: d.total as u64,
    }
}
