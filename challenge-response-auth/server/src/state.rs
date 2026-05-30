//! In-memory session store for pending challenges (replay resistance after success).

use std::collections::HashMap;
use std::sync::Arc;

use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use uuid::Uuid;

use chal_auth_shared::{unix_timestamp_ms_now, NONCE_LEN};

/// Server-side state for one challenge issuance.
#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub nonce: [u8; NONCE_LEN],
    pub challenge_timestamp_ms: i64,
    pub created_at_wall_ms: i64,
}

/// Pending entries keyed by the `session_id` issued in `/v1/challenge`.
#[derive(Clone)]
pub struct ChallengeStore {
    inner: Arc<RwLock<HashMap<Uuid, PendingChallenge>>>,
    ttl_ms: i64,
}

impl ChallengeStore {
    #[must_use]
    pub fn new(ttl_ms: i64) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl_ms,
        }
    }

    /// Delete very old rows to avoid unbounded RAM growth (best-effort).
    async fn prune(&self) {
        let Ok(now_ms) = unix_timestamp_ms_now() else {
            return;
        };
        let max_age = self.ttl_ms.saturating_mul(3).max(self.ttl_ms);
        let mut map = self.inner.write().await;
        map.retain(|_, pending| {
            now_ms.saturating_sub(pending.created_at_wall_ms) <= max_age
        });
    }

    pub async fn record_challenge(&self, id: Uuid, pending: PendingChallenge) {
        self.prune().await;
        self.inner.write().await.insert(id, pending);
    }

    pub async fn get_pending(&self, id: &Uuid) -> Option<PendingChallenge> {
        self.prune().await;
        self.inner.read().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &Uuid) -> Option<PendingChallenge> {
        self.inner.write().await.remove(id)
    }

    /// Nonce equality (constant-time) and exact timestamp echo checks.
    /// Does **not** include MAC verification or TTL/skew checks (caller).
    #[must_use]
    pub fn nonce_and_timestamp_consistent_with(
        pending: &PendingChallenge,
        nonce_bytes: &[u8; NONCE_LEN],
        echoed_challenge_timestamp_ms: i64,
    ) -> bool {
        if !bool::from(nonce_bytes.ct_eq(&pending.nonce)) {
            return false;
        }
        if pending.challenge_timestamp_ms != echoed_challenge_timestamp_ms {
            return false;
        }
        true
    }
}
