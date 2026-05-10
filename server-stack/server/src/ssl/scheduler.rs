//! Daily (configurable) SSL check + auto-renew loop.

use super::config::load_ssl_config;
use super::service::SslService;
use deploy_db::DbStore;
use deploy_proto::deploy::SslCheckAndRenewRequest;
use std::sync::Arc;
use tracing::{error, info};

/// Spawn a background task that calls [`SslService::check_and_renew`] on an interval.
/// No-op if metadata DB is disabled. Set `SSL_ENABLE_SCHEDULER=0` to disable.
pub fn spawn_ssl_scheduler(db: Option<Arc<DbStore>>) {
    let Some(db) = db else {
        return;
    };
    if std::env::var("SSL_ENABLE_SCHEDULER")
        .ok()
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
    {
        return;
    }
    let interval_secs = load_ssl_config().check_interval_secs.max(60);
    let db2 = db.clone();
    tokio::spawn(async move {
        let mut intv =
            tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        // First tick completes immediately; skip to avoid double-run at startup
        intv.tick().await;
        loop {
            intv.tick().await;
            let svc = SslService::new(db2.clone());
            let req = SslCheckAndRenewRequest {
                force_all: false,
                project_id: String::new(),
            };
            match svc.check_and_renew(req).await {
                Ok(r) => {
                    info!(
                        checked = r.checked,
                        attempted = r.attempted_renew,
                        failed = r.failed,
                        "ssl scheduler tick"
                    );
                }
                Err(e) => {
                    error!(%e, "ssl scheduler check_and_renew");
                }
            }
        }
    });
}
