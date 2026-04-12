//! Local counters for a board session (bytes + wall time).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub struct TunnelMetrics {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub wall_ms: AtomicU64,
    started: Instant,
}

impl TunnelMetrics {
    pub fn new() -> Self {
        Self {
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            wall_ms: AtomicU64::new(0),
            started: Instant::now(),
        }
    }

    pub fn add_in(&self, n: u64) {
        self.bytes_in.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_out(&self, n: u64) {
        self.bytes_out.fetch_add(n, Ordering::Relaxed);
    }

    pub fn finalize_wall_ms(&self) {
        self.wall_ms
            .store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
}

impl Default for TunnelMetrics {
    fn default() -> Self {
        Self::new()
    }
}
