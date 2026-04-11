//! Local system monitoring: HTTP API on loopback + optional WebSocket stream.
//!
//! See `/api/v1/monitoring/*` — used by the desktop web UI.

mod alerts;
mod collector;
mod history;
mod server;
mod sqlite_store;
mod types;

pub use server::{monitoring_api_base, monitoring_set_economy_mode, spawn_monitoring_server};
pub use types::MonitoringOverview;
