//! Priority queue for tunnel admission when `DEPLOY_PROXY_MAX_CONCURRENT_TUNNELS` is set.

use crate::metrics_http::ProxyTunnelMetrics;
use crate::tunnel_registry::TunnelRedis;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tonic::Status;
use uuid::Uuid;

#[derive(Debug)]
struct Waiter {
    priority: i32,
    enqueue_ms: u64,
    id: Uuid,
    notify: Arc<Notify>,
}

pub struct TunnelAdmission {
    max: Option<usize>,
    active: AtomicUsize,
    waiters: Mutex<Vec<Waiter>>,
    metrics: Arc<ProxyTunnelMetrics>,
}

pub struct AdmissionGuard {
    admission: Arc<TunnelAdmission>,
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.admission.release_slot();
    }
}

impl TunnelAdmission {
    pub fn new(max: Option<usize>, metrics: Arc<ProxyTunnelMetrics>) -> Arc<Self> {
        Arc::new(Self {
            max,
            active: AtomicUsize::new(0),
            waiters: Mutex::new(Vec::new()),
            metrics,
        })
    }

    fn try_take_slot(self: &Arc<Self>) -> bool {
        let Some(max) = self.max else {
            let _ = self.active.fetch_add(1, AtomicOrdering::AcqRel);
            return true;
        };
        loop {
            let cur = self.active.load(AtomicOrdering::Acquire);
            if cur >= max {
                return false;
            }
            if self
                .active
                .compare_exchange_weak(cur, cur + 1, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Acquire a tunnel slot; higher `priority` is scheduled first when waiting.
    pub async fn acquire(
        self: &Arc<Self>,
        priority: i32,
        tunnel_id: Uuid,
        redis: Option<&TunnelRedis>,
        wait_timeout: Option<Duration>,
    ) -> Result<AdmissionGuard, Status> {
        loop {
            if self.try_take_slot() {
                return Ok(AdmissionGuard {
                    admission: Arc::clone(self),
                });
            }

            if self.max.is_none() {
                self.active.fetch_add(1, AtomicOrdering::AcqRel);
                return Ok(AdmissionGuard {
                    admission: Arc::clone(self),
                });
            }

            self.metrics
                .tunnel_waiters_current
                .fetch_add(1, AtomicOrdering::Relaxed);
            self.metrics
                .tunnel_wait_enqueue_total
                .fetch_add(1, AtomicOrdering::Relaxed);

            let notify = Arc::new(Notify::new());
            let waiter = Waiter {
                priority,
                enqueue_ms: Instant::now().elapsed().as_millis() as u64,
                id: tunnel_id,
                notify: notify.clone(),
            };

            self.waiters.lock().push(waiter);

            if let Some(r) = redis {
                if let Err(e) = r.wait_zadd(&tunnel_id, priority).await {
                    tracing::warn!(error = %e, "redis wait_zadd");
                }
            }

            let wait_fut = notify.notified();
            let timed = if let Some(t) = wait_timeout {
                tokio::time::timeout(t, wait_fut).await
            } else {
                Ok(wait_fut.await)
            };

            if timed.is_err() {
                if let Some(r) = redis {
                    let _ = r.wait_zrem(&tunnel_id).await;
                }
                self.waiters.lock().retain(|w| w.id != tunnel_id);
                self.metrics
                    .tunnel_waiters_current
                    .fetch_sub(1, AtomicOrdering::Relaxed);
                self.metrics
                    .tunnel_wait_timeout_total
                    .fetch_add(1, AtomicOrdering::Relaxed);
                return Err(Status::resource_exhausted(
                    "tunnel admission wait timeout (DEPLOY_PROXY_TUNNEL_WAIT_TIMEOUT_SEC)",
                ));
            }

            if let Some(r) = redis {
                let _ = r.wait_zrem(&tunnel_id).await;
            }

            self.metrics
                .tunnel_waiters_current
                .fetch_sub(1, AtomicOrdering::Relaxed);
        }
    }

    fn release_slot(&self) {
        self.active.fetch_sub(1, AtomicOrdering::AcqRel);
        let w = {
            let mut q = self.waiters.lock();
            if q.is_empty() {
                None
            } else {
                let best = q
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        a.priority
                            .cmp(&b.priority)
                            .then_with(|| a.enqueue_ms.cmp(&b.enqueue_ms).reverse())
                    })
                    .map(|(i, _)| i);
                best.map(|i| q.remove(i))
            }
        };
        if let Some(w) = w {
            w.notify.notify_one();
        }
    }
}

pub fn max_concurrent_from_env() -> Option<usize> {
    let v = std::env::var("DEPLOY_PROXY_MAX_CONCURRENT_TUNNELS").ok()?;
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    let n: usize = t.parse().ok()?;
    if n == 0 {
        return None;
    }
    Some(n)
}

pub fn wait_timeout_from_env() -> Option<Duration> {
    let v = std::env::var("DEPLOY_PROXY_TUNNEL_WAIT_TIMEOUT_SEC").ok()?;
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    let sec: u64 = t.parse().ok()?;
    if sec == 0 {
        return None;
    }
    Some(Duration::from_secs(sec))
}

pub fn checkpoint_interval_from_env() -> Duration {
    let def = 1800u64;
    let sec = std::env::var("DEPLOY_PROXY_TUNNEL_CHECKPOINT_SEC")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(def);
    let sec = sec.max(60);
    Duration::from_secs(sec)
}
