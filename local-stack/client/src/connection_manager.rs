//! Per-endpoint gRPC channel reuse, optional concurrency limit, and semaphore.

use crate::grpc_transport::{endpoint_channel, grpc_channel_cache_key};
use crate::settings::BoardConfig;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tonic::transport::Channel;

#[derive(Clone)]
pub struct ConnectionManager {
    inner: Arc<Mutex<Inner>>,
    default_max: usize,
}

struct Inner {
    semaphores: HashMap<String, Arc<Semaphore>>,
    channels: HashMap<String, Channel>,
}

impl ConnectionManager {
    pub fn new(default_max: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                semaphores: HashMap::new(),
                channels: HashMap::new(),
            })),
            default_max: default_max.max(1),
        }
    }

    pub fn semaphore_for(&self, endpoint: &str, max: Option<usize>) -> Arc<Semaphore> {
        let cap = max.unwrap_or(self.default_max).max(1);
        let mut g = self.inner.lock();
        g.semaphores
            .entry(endpoint.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(cap)))
            .clone()
    }

    /// Cached lazy `Channel` for this endpoint + board TLS settings; creates on first use.
    pub fn channel_for(
        &self,
        endpoint: &str,
        board: &BoardConfig,
    ) -> Result<Channel, String> {
        let key = grpc_channel_cache_key(endpoint, board);
        let mut g = self.inner.lock();
        if let Some(ch) = g.channels.get(&key) {
            return Ok(ch.clone());
        }
        let ch = endpoint_channel(endpoint, board)?;
        g.channels.insert(key, ch.clone());
        Ok(ch)
    }

    /// Drop cached channel (e.g. after RPC failure); next call rebuilds.
    pub fn invalidate_channel(&self, endpoint: &str, board: &BoardConfig) {
        let key = grpc_channel_cache_key(endpoint, board);
        let mut g = self.inner.lock();
        g.channels.remove(&key);
    }
}
