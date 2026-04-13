//! Delta flush of managed proxy session usage to SQLite (checkpoints + final).

use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Optional periodic SQLite + Redis updates for a managed tunnel (RAW or wire bridge).
#[derive(Clone)]
pub struct ManagedTunnelCheckpoint {
    pub db: Arc<deploy_db::DbStore>,
    pub session_id: String,
    pub client_pubkey: String,
    pub counters: Arc<Mutex<TunnelFlushCounters>>,
    pub interval: Duration,
    pub redis: Option<(Arc<crate::tunnel_registry::TunnelRedis>, uuid::Uuid)>,
    pub tunnel_id: uuid::Uuid,
    pub wire_mode: i32,
    pub priority: i32,
    pub stream_correlation_id: String,
}

/// Per-tunnel counters since last successful DB flush (this gRPC stream only).
#[derive(Debug, Clone, Default)]
pub struct TunnelFlushCounters {
    last_flushed_bi: u64,
    last_flushed_bo: u64,
    last_flushed_active_ms: u64,
}

impl TunnelFlushCounters {
    /// Returns `(delta_in, delta_out, delta_active_ms)` and advances baseline.
    pub fn take_deltas(&mut self, bi: u64, bo: u64, active_ms: u64) -> (u64, u64, i64) {
        let dbi = bi.saturating_sub(self.last_flushed_bi);
        let dbo = bo.saturating_sub(self.last_flushed_bo);
        let da = (active_ms as i128)
            .saturating_sub(self.last_flushed_active_ms as i128)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        self.last_flushed_bi = bi;
        self.last_flushed_bo = bo;
        self.last_flushed_active_ms = active_ms;
        (dbi, dbo, da)
    }
}

/// Flush deltas for a managed session; no-op if deltas are all zero.
pub async fn flush_managed_session_deltas(
    db: &deploy_db::DbStore,
    session_id: &str,
    client_pubkey_b64: &str,
    bi: u64,
    bo: u64,
    active_ms: u64,
    counters: &mut TunnelFlushCounters,
    last_activity_at: DateTime<Utc>,
    set_first_open_if_null: Option<DateTime<Utc>>,
) -> Result<(), deploy_db::DbError> {
    let (dbi, dbo, da) = counters.take_deltas(bi, bo, active_ms);
    if dbi == 0 && dbo == 0 && da == 0 {
        return Ok(());
    }
    db.increment_grpc_proxy_session_traffic(
        session_id,
        client_pubkey_b64,
        dbi,
        dbo,
        da,
        last_activity_at,
        set_first_open_if_null,
    )
    .await
}

/// Final delta flush at tunnel close (remainder after last checkpoint).
pub async fn flush_managed_tunnel_end(
    cp: &ManagedTunnelCheckpoint,
    bi: u64,
    bo: u64,
    active_ms: u64,
) -> Result<(), deploy_db::DbError> {
    let now = Utc::now();
    let mut c = cp.counters.lock().await;
    flush_managed_session_deltas(
        cp.db.as_ref(),
        &cp.session_id,
        &cp.client_pubkey,
        bi,
        bo,
        active_ms,
        &mut *c,
        now,
        Some(now),
    )
    .await
}

/// Start background checkpoint task; call `tx.send(true)` then `jh.abort()` (or await stop) before final flush.
pub fn spawn_managed_checkpoint<F>(
    cp: ManagedTunnelCheckpoint,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    read_active_ms: F,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::watch::Sender<bool>,
)
where
    F: Fn() -> u64 + Send + 'static,
{
    let (tx, rx) = tokio::sync::watch::channel(false);
    let jh = tokio::spawn(async move {
        run_checkpoint_loop(
            cp.interval,
            cp.db,
            cp.session_id,
            cp.client_pubkey,
            cp.counters,
            bytes_in,
            bytes_out,
            read_active_ms,
            cp.redis,
            cp.tunnel_id,
            cp.wire_mode,
            cp.priority,
            cp.stream_correlation_id,
            rx,
        )
        .await
    });
    (jh, tx)
}

/// Managed session checkpoint: periodic delta flush + optional Redis mirror.
pub async fn run_checkpoint_loop<F>(
    interval: Duration,
    db: Arc<deploy_db::DbStore>,
    session_id: String,
    client_pubkey: String,
    counters: Arc<Mutex<TunnelFlushCounters>>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    mut read_active_ms: F,
    redis: Option<(Arc<crate::tunnel_registry::TunnelRedis>, uuid::Uuid)>,
    tunnel_id: uuid::Uuid,
    wire_mode: i32,
    priority: i32,
    stream_correlation_id: String,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    F: FnMut() -> u64 + Send + 'static,
{
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;

    loop {
        tokio::select! {
            biased;
            res = shutdown.changed() => {
                if res.is_err() {
                    break;
                }
                if *shutdown.borrow() {
                    break;
                }
            }
            _ = tick.tick() => {
                let bi = bytes_in.load(Ordering::Relaxed);
                let bo = bytes_out.load(Ordering::Relaxed);
                let am = read_active_ms();
                let now = Utc::now();
                let last_cp = {
                    let mut c = counters.lock().await;
                    if let Err(e) = flush_managed_session_deltas(
                        db.as_ref(),
                        &session_id,
                        &client_pubkey,
                        bi,
                        bo,
                        am,
                        &mut *c,
                        now,
                        Some(now),
                    )
                    .await
                    {
                        tracing::error!(%e, "grpc proxy session checkpoint");
                    }
                    c.last_flushed_active_ms
                };
                if let Some((ref r, _)) = redis {
                    let fields = crate::tunnel_registry::redis_fields_snapshot(
                        &tunnel_id,
                        Some(&session_id),
                        Some(&client_pubkey),
                        &stream_correlation_id,
                        wire_mode,
                        priority,
                        am,
                        bi,
                        bo,
                        last_cp,
                    );
                    let refs: Vec<(&str, &str)> = fields
                        .iter()
                        .map(|(a, b)| (a.as_str(), b.as_str()))
                        .collect();
                    if let Err(e) = r.update_fields(&tunnel_id, &refs).await {
                        tracing::warn!(error = %e, "redis tunnel checkpoint update");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod flush_tests {
    use super::TunnelFlushCounters;

    #[test]
    fn checkpoint_deltas_then_final_equal_full_tunnel_totals() {
        let mut c = TunnelFlushCounters::default();
        let (a, b, am) = c.take_deltas(100, 50, 2000);
        assert_eq!((a, b, am), (100, 50, 2000));
        let (a2, b2, am2) = c.take_deltas(300, 120, 5000);
        assert_eq!((a2, b2, am2), (200, 70, 3000));
        let (a3, b3, am3) = c.take_deltas(300, 120, 5000);
        assert_eq!((a3, b3, am3), (0, 0, 0), "no duplicate flush");
    }
}
