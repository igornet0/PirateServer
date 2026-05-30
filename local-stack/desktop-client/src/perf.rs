//! Lightweight performance instrumentation (enable with `PIRATE_DESKTOP_PERF=1`).

use std::time::Instant;
use tracing::{info, warn};

pub fn perf_enabled() -> bool {
    std::env::var("PIRATE_DESKTOP_PERF")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Log command duration and optional response byte length when perf mode is on.
pub fn log_command(command: &str, started: Instant, response_bytes: Option<usize>) {
    if !perf_enabled() {
        return;
    }
    let ms = started.elapsed().as_millis();
    match response_bytes {
        Some(n) => info!(command, ms, bytes = n, "tauri command"),
        None => info!(command, ms, "tauri command"),
    }
}

pub fn log_http_client_stats() {
    if !perf_enabled() {
        return;
    }
    let n = crate::http_client::blocking_client_acquire_count();
    info!(blocking_acquires = n, "shared HTTP client stats");
}

pub fn warn_slow_command(command: &str, started: Instant, threshold_ms: u128) {
    let ms = started.elapsed().as_millis();
    if ms >= threshold_ms {
        warn!(command, ms, "slow tauri command");
    }
}

/// JSON snapshot for baseline / regression checks (also exposed as a Tauri command).
pub fn desktop_perf_snapshot_json() -> Result<String, String> {
    let samples = crate::monitoring::sample_row_count().unwrap_or(-1);
    let blocking_acquires = crate::http_client::blocking_client_acquire_count();
    serde_json::to_string(&serde_json::json!({
        "samplesRowCount": samples,
        "blockingHttpAcquires": blocking_acquires,
    }))
    .map_err(|e| e.to_string())
}
