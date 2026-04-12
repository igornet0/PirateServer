//! Per-endpoint gRPC channel reuse and optional concurrency limit.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct ConnectionManager {
    inner: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    default_max: usize,
}

impl ConnectionManager {
    pub fn new(default_max: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            default_max: default_max.max(1),
        }
    }

    pub fn semaphore_for(&self, endpoint: &str, max: Option<usize>) -> Arc<Semaphore> {
        let cap = max.unwrap_or(self.default_max).max(1);
        let mut g = self.inner.lock();
        g.entry(endpoint.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(cap)))
            .clone()
    }
}
