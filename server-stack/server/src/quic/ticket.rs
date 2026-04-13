//! Short-lived single-use tickets for QUIC stream auth.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::context::QuicRawContext;

#[derive(Clone)]
pub struct QuicTicketStore {
    inner: std::sync::Arc<Mutex<HashMap<[u8; 32], (QuicRawContext, Instant)>>>,
    ttl: Duration,
}

impl QuicTicketStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    pub async fn insert(&self, ticket: [u8; 32], grant: QuicRawContext) {
        let mut g = self.inner.lock().await;
        g.retain(|_, (_, t)| t.elapsed() < self.ttl);
        g.insert(ticket, (grant, Instant::now()));
    }

    pub async fn take(&self, ticket: &[u8]) -> Option<QuicRawContext> {
        if ticket.len() != 32 {
            return None;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(ticket);
        let mut g = self.inner.lock().await;
        g.retain(|_, (_, t)| t.elapsed() < self.ttl);
        if let Some((grant, issued)) = g.remove(&key) {
            if issued.elapsed() > self.ttl {
                return None;
            }
            return Some(grant);
        }
        None
    }
}
