//! FIFO queue of accepted TCP sockets for listener profiles.

use crate::types::GlobalStats;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::Notify;

#[derive(Debug)]
struct Pending {
    stream: TcpStream,
    enqueued: Instant,
}

#[derive(Debug)]
pub struct StreamQueue {
    cap: usize,
    offer_ttl: Duration,
    inner: Mutex<VecDeque<Pending>>,
    notify: Notify,
}

impl StreamQueue {
    pub fn new(cap: usize, offer_ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            cap,
            offer_ttl,
            inner: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        })
    }

    /// Return false if queue full (caller should drop TcpStream).
    pub fn try_enqueue(self: &Arc<Self>, g: &GlobalStats, stream: TcpStream) -> bool {
        self.prune_expired(g);
        {
            let mut mq = self.inner.lock();
            if mq.len() >= self.cap {
                g.listener_dropped_queue_full
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
            mq.push_back(Pending {
                stream,
                enqueued: Instant::now(),
            });
        }
        g.listener_accepts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.notify.notify_waiters();
        true
    }

    fn prune_expired(&self, g: &GlobalStats) {
        let now = Instant::now();
        let mut mq = self.inner.lock();
        let ttl = self.offer_ttl;
        while let Some(front) = mq.front() {
            if now.duration_since(front.enqueued) > ttl {
                let _ = mq.pop_front();
                g.listener_dropped_expired
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                break;
            }
        }
    }

    /// Blocks until a TCP stream is available or `wait` elapses.
    pub async fn dequeue(&self, wait: Duration, g: &GlobalStats) -> Option<TcpStream> {
        tokio::time::timeout(wait, async {
            loop {
                self.prune_expired(g);
                if let Some(p) = {
                    let mut mq = self.inner.lock();
                    mq.pop_front()
                } {
                    return Some(p.stream);
                }
                self.notify.notified().await;
            }
        })
        .await
        .ok()
        .flatten()
    }
}
