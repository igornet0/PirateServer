//! Shared `reqwest` blocking clients (connection pooling, single build per process).

use reqwest::blocking::Client;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

static BLOCKING: LazyLock<Client> =
    LazyLock::new(|| build_blocking(Duration::from_secs(30)).expect("blocking HTTP client"));

static HOST_AGENT: LazyLock<Client> =
    LazyLock::new(|| build_blocking(Duration::from_secs(600)).expect("host-agent HTTP client"));

static STACK_TUN: LazyLock<Client> =
    LazyLock::new(|| build_blocking(Duration::from_secs(30)).expect("stack-tun HTTP client"));

static UPLOAD: LazyLock<Client> =
    LazyLock::new(|| build_blocking(Duration::from_secs(86400)).expect("upload HTTP client"));

static ASYNC: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(45))
        .pool_max_idle_per_host(8)
        .build()
        .expect("async HTTP client")
});

/// Dev/diagnostic: how many times a shared client was requested (not per-request builders).
static BLOCKING_ACQUIRES: AtomicU64 = AtomicU64::new(0);

const UA: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

fn build_blocking(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .user_agent(UA)
        .timeout(timeout)
        .pool_max_idle_per_host(8)
        .build()
        .map_err(|e| e.to_string())
}

/// Default control-api / general HTTP (30s).
pub fn blocking_client() -> Result<&'static Client, String> {
    BLOCKING_ACQUIRES.fetch_add(1, Ordering::Relaxed);
    Ok(&BLOCKING)
}

/// Short probes (`/health`, etc.).
pub fn blocking_client_short() -> Result<&'static Client, String> {
    blocking_client()
}

/// Host-agent uploads (long timeout).
pub fn host_agent_client() -> Result<&'static Client, String> {
    Ok(&HOST_AGENT)
}

/// stack-tun-api REST.
pub fn stack_tun_client() -> Result<&'static Client, String> {
    Ok(&STACK_TUN)
}

/// Large artifact / storage uploads (long timeout, separate pool).
pub fn blocking_client_upload() -> Result<&'static Client, String> {
    Ok(&UPLOAD)
}

/// Async control-api fan-out (`fetch_server_projects_overview`, etc.).
pub fn async_client() -> Result<&'static reqwest::Client, String> {
    Ok(&ASYNC)
}

/// Baseline metric for optimization work (logged when `PIRATE_DESKTOP_PERF=1`).
pub fn blocking_client_acquire_count() -> u64 {
    BLOCKING_ACQUIRES.load(Ordering::Relaxed)
}
