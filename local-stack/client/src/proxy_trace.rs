//! In-memory ring buffer for CONNECT proxy trace lines (desktop UI).

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Shorten gRPC URL for log display (scheme stripped, max width).
pub fn compact_grpc_endpoint_for_log(s: &str) -> String {
    let t = s.trim();
    let without = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    const MAX: usize = 64;
    if without.len() > MAX {
        format!("{}…", &without[..MAX.saturating_sub(1)])
    } else {
        without.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTraceEntry {
    pub timestamp_ms: u64,
    pub client_addr: String,
    pub target: String,
    pub decision: String,
    pub route: String,
    pub result: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Ring buffer shared between `run_board` tasks and the desktop shell.
pub struct ProxyTraceBuffer {
    inner: Mutex<VecDeque<ProxyTraceEntry>>,
    cap: usize,
}

impl ProxyTraceBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            cap: cap.max(1),
        }
    }

    pub fn push(&self, entry: ProxyTraceEntry) {
        let mut g = self.inner.lock();
        while g.len() >= self.cap {
            g.pop_front();
        }
        g.push_back(entry);
    }

    /// Append one line; `ok` maps to `result` "ok" / "fail".
    pub fn log(
        &self,
        client_addr: impl Into<String>,
        target: impl Into<String>,
        decision: impl Into<String>,
        route: impl Into<String>,
        ok: bool,
        detail: Option<String>,
    ) {
        self.push(ProxyTraceEntry {
            timestamp_ms: now_ms(),
            client_addr: client_addr.into(),
            target: target.into(),
            decision: decision.into(),
            route: route.into(),
            result: if ok {
                "ok".to_string()
            } else {
                "fail".to_string()
            },
            detail,
        });
    }

    pub fn snapshot(&self) -> Vec<ProxyTraceEntry> {
        self.inner.lock().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
    }
}

pub fn trace_log(
    trace: &Option<Arc<ProxyTraceBuffer>>,
    client_addr: impl Into<String>,
    target: impl Into<String>,
    decision: impl Into<String>,
    route: impl Into<String>,
    ok: bool,
    detail: Option<String>,
) {
    if let Some(t) = trace {
        t.log(client_addr, target, decision, route, ok, detail);
    }
}
