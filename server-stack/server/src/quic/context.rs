//! Per-tunnel state moved from gRPC task to QUIC accept path.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use crate::metrics_http::ProxyTunnelMetrics;
use crate::proxy_session;
use crate::tunnel_admission::AdmissionGuard;
use crate::tunnel_flush::ManagedTunnelCheckpoint;
use deploy_db::DbStore;
use uuid::Uuid;

/// Same semantics as the inline `ActiveTracker` in `deploy_service` raw tunnel.
pub struct ActiveTracker {
    pub last: Option<std::time::Instant>,
    pub idle: std::time::Duration,
    pub accum_ms: u64,
}

impl ActiveTracker {
    pub fn new(idle_secs: u64) -> Self {
        Self {
            last: None,
            idle: std::time::Duration::from_secs(idle_secs.max(1)),
            accum_ms: 0,
        }
    }

    pub fn bump(&mut self) {
        let now = std::time::Instant::now();
        if let Some(prev) = self.last {
            let g = now.saturating_duration_since(prev);
            if g <= self.idle {
                self.accum_ms += g.as_millis() as u64;
            }
        }
        self.last = Some(now);
    }
}

#[allow(dead_code)]
pub struct QuicRawContext {
    pub expected_host: String,
    pub expected_port: u16,
    pub tunnel_id: Uuid,
    pub admission_guard: AdmissionGuard,
    pub managed_checkpoint: Option<ManagedTunnelCheckpoint>,
    pub bytes_in: Arc<AtomicU64>,
    pub bytes_out: Arc<AtomicU64>,
    pub metrics: Arc<ProxyTunnelMetrics>,
    pub db_opt: Option<Arc<DbStore>>,
    pub client_pubkey_for_traffic: Option<String>,
    pub stream_correlation_id: String,
    pub session_id_for_task: Option<String>,
    pub pk_for_task: Option<String>,
    pub policy_for_task: Option<proxy_session::StoredPolicy>,
    pub base_in: u64,
    pub base_out: u64,
    pub base_active_ms: u64,
    pub active: Arc<StdMutex<ActiveTracker>>,
    pub completion: tokio::sync::oneshot::Sender<()>,
}
