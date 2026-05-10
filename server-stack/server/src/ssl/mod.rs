//! ACME / Certbot integration (host-level; metadata in `deploy-db`).

pub mod certbot;
pub mod config;
pub mod postcheck;
pub mod scheduler;
pub mod service;

pub use scheduler::spawn_ssl_scheduler;
pub use service::SslService;
