//! Local HTTP ingest for display stream frames (consumer side).

mod server;

pub use server::{display_ingest_api_base, display_ingest_url, spawn_display_ingest_server};
