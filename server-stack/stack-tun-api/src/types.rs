//! Persisted tunnel profiles and API DTOs.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelRole {
    Listen,
    Connector,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TunnelMode {
    #[default]
    TcpRelay,
    #[serde(alias = "requestBus")]
    RequestBus,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TunnelLinkKind {
    #[default]
    Local,
    #[serde(alias = "publicAuth")]
    PublicAuth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BusRouteDecisionKind {
    Allow,
    Deny,
    Forward,
    #[serde(rename = "localHandle")]
    LocalHandle,
    /// Enqueue asynchronous task for TaskQueue worker (no synchronous HTTP upstream).
    #[serde(rename = "queue")]
    Queue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTunRouteRule {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_contains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub decision: BusRouteDecisionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBusJournalEntry {
    pub ts_unix_ms: i64,
    #[serde(default)]
    pub hop_id: u32,
    pub request_id: String,
    pub trace_id: String,
    pub source_node_id: String,
    pub target_node_id: String,
    pub profile_id: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub status: u32,
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub bytes_in: usize,
    pub bytes_out: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelProfile {
    pub id: String,
    #[serde(default = "default_name")]
    pub name: String,
    pub role: TunnelRole,
    /// `tcpRelay` (default): public TCP queued relay. `requestBus`: policy + journaling on this profile.
    #[serde(default)]
    pub mode: TunnelMode,
    #[serde(default)]
    pub link_kind: TunnelLinkKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_node_id: Option<String>,
    #[serde(default)]
    pub route_tags: Vec<String>,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_priority: Option<i32>,
    /// When rules do not match, apply this (`allow` proxies as-is).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_bus_decision: Option<BusRouteDecisionKind>,
    /// Listener bind (e.g. `0.0.0.0:9000`). Required when `role = Listen` unless `mode = requestBus`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_addr: Option<String>,
    /// Remote stack-tun gRPC base (`http://host:9381` or `http://127.0.0.1:9381`). Connector only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_grpc_endpoint: Option<String>,
    /// Listener-side profile id to pull streams for. Connector only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_profile_id: Option<String>,
    /// Local upstream for connector relays (`127.0.0.1`, service on PC/lan host).
    #[serde(default)]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
    #[serde(default = "default_max_pending")]
    pub max_pending_streams: usize,
    #[serde(default = "default_offer_ttl_secs")]
    pub stream_offer_ttl_secs: u64,
    #[serde(default = "default_pull_wait_ms")]
    pub pull_wait_ms: u64,
    /// If empty, any peer in authorized_peers.json may pull from this listener profile.
    #[serde(default)]
    pub connector_allow_pubkey_b64: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Max queued tasks awaiting claim (defaults to [`max_pending_streams`] when unset).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_pending_tasks: Option<usize>,
    /// Max seconds a task stays in `queued` before expiring (defaults to [`stream_offer_ttl_secs`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_queue_ttl_secs: Option<u64>,
    /// Lease for `claimed` / `running` before re-queue (defaults to 300s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_claim_lease_secs: Option<u64>,
}

fn default_max_pending() -> usize {
    128
}

fn default_offer_ttl_secs() -> u64 {
    300
}

fn default_pull_wait_ms() -> u64 {
    30_000
}

fn default_name() -> String {
    "tunnel".into()
}

fn default_true() -> bool {
    true
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistRoot {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub profiles: Vec<TunnelProfile>,
    #[serde(default)]
    pub routes: Vec<StackTunRouteRule>,
}

impl TunnelProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("profile id empty".into());
        }
        match self.role {
            TunnelRole::Listen => {
                let need_tcp_listen = !(self.mode == TunnelMode::RequestBus);
                if need_tcp_listen
                    && self
                        .listen_addr
                        .as_ref()
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true)
                {
                    return Err(
                        "listen profiles require listen_addr unless mode is requestBus".into(),
                    );
                }
            }
            TunnelRole::Connector => {
                if self
                    .remote_grpc_endpoint
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err("connector profiles require remote_grpc_endpoint".into());
                }
                if self
                    .listen_profile_id
                    .as_ref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err("connector profiles require listen_profile_id".into());
                }
                if self.target_host.trim().is_empty() {
                    return Err("connector target_host empty".into());
                }
                if self.target_port == 0 {
                    return Err("connector target_port invalid".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatsSnapshot {
    pub listener_accepts: u64,
    pub listener_dropped_queue_full: u64,
    pub listener_dropped_expired: u64,
    pub connector_pulls: u64,
    pub relay_completed: u64,
    pub relay_errors: u64,
    pub request_bus_received: u64,
    pub request_bus_blocked: u64,
    pub request_bus_completed: u64,
    pub request_bus_errors: u64,
    pub task_queue_submitted: u64,
    pub task_queue_claimed: u64,
    pub task_queue_claim_timeout: u64,
    pub task_queue_status_updates: u64,
    pub task_queue_completed: u64,
    pub task_queue_failed: u64,
    pub task_queue_expired: u64,
    pub task_queue_lease_requeue: u64,
}

#[derive(Default)]
pub struct GlobalStats {
    pub listener_accepts: AtomicU64,
    pub listener_dropped_queue_full: AtomicU64,
    pub listener_dropped_expired: AtomicU64,
    pub connector_pulls: AtomicU64,
    pub relay_completed: AtomicU64,
    pub relay_errors: AtomicU64,
    pub request_bus_received: AtomicU64,
    pub request_bus_blocked: AtomicU64,
    pub request_bus_completed: AtomicU64,
    pub request_bus_errors: AtomicU64,
    pub task_queue_submitted: AtomicU64,
    pub task_queue_claimed: AtomicU64,
    pub task_queue_claim_timeout: AtomicU64,
    pub task_queue_status_updates: AtomicU64,
    pub task_queue_completed: AtomicU64,
    pub task_queue_failed: AtomicU64,
    pub task_queue_expired: AtomicU64,
    pub task_queue_lease_requeue: AtomicU64,
}

impl GlobalStats {
    pub fn snapshot(&self) -> TunnelStatsSnapshot {
        TunnelStatsSnapshot {
            listener_accepts: self.listener_accepts.load(Ordering::Relaxed),
            listener_dropped_queue_full: self.listener_dropped_queue_full.load(Ordering::Relaxed),
            listener_dropped_expired: self.listener_dropped_expired.load(Ordering::Relaxed),
            connector_pulls: self.connector_pulls.load(Ordering::Relaxed),
            relay_completed: self.relay_completed.load(Ordering::Relaxed),
            relay_errors: self.relay_errors.load(Ordering::Relaxed),
            request_bus_received: self.request_bus_received.load(Ordering::Relaxed),
            request_bus_blocked: self.request_bus_blocked.load(Ordering::Relaxed),
            request_bus_completed: self.request_bus_completed.load(Ordering::Relaxed),
            request_bus_errors: self.request_bus_errors.load(Ordering::Relaxed),
            task_queue_submitted: self.task_queue_submitted.load(Ordering::Relaxed),
            task_queue_claimed: self.task_queue_claimed.load(Ordering::Relaxed),
            task_queue_claim_timeout: self.task_queue_claim_timeout.load(Ordering::Relaxed),
            task_queue_status_updates: self.task_queue_status_updates.load(Ordering::Relaxed),
            task_queue_completed: self.task_queue_completed.load(Ordering::Relaxed),
            task_queue_failed: self.task_queue_failed.load(Ordering::Relaxed),
            task_queue_expired: self.task_queue_expired.load(Ordering::Relaxed),
            task_queue_lease_requeue: self.task_queue_lease_requeue.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub ts_unix_ms: i64,
    pub level: String,
    pub message: String,
}
