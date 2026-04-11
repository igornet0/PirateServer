//! Nonce replay protection: reject duplicate nonces within a time window.

use parking_lot::Mutex;
use std::collections::HashMap;

const WINDOW_MS: i64 = 600_000;
const MAX_ENTRIES: usize = 50_000;

#[derive(Default)]
pub struct NonceTracker {
    inner: Mutex<HashMap<String, i64>>,
}

impl NonceTracker {
    pub fn check_and_insert(&self, ts_ms: i64, nonce: &str) -> Result<(), super::AuthError> {
        if nonce.is_empty() || nonce.len() > 128 {
            return Err(super::AuthError::InvalidMetadata("nonce".into()));
        }
        let mut m = self.inner.lock();
        m.retain(|_, &mut t| (ts_ms - t).abs() <= WINDOW_MS);
        if m.len() >= MAX_ENTRIES {
            m.clear();
        }
        let key = nonce.to_string();
        if m.contains_key(&key) {
            return Err(super::AuthError::ReplayNonce);
        }
        m.insert(key, ts_ms);
        Ok(())
    }
}
