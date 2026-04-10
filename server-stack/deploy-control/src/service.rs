use crate::types::{HistoryView, ReleasesView, StatusView};
use deploy_core::list_release_versions;
use deploy_db::DbStore;
use deploy_proto::deploy::deploy_service_client::DeployServiceClient;
use deploy_proto::deploy::StatusRequest;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("grpc: {0}")]
    Grpc(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] deploy_db::DbError),
}

/// Aggregates deploy-server gRPC, optional DB snapshot, and filesystem layout.
pub struct ControlPlane {
    deploy_root: PathBuf,
    grpc_endpoint: String,
    /// Shared with background reconcile; same pool as used for history/snapshot reads.
    pub db: Option<Arc<DbStore>>,
}

impl ControlPlane {
    pub fn new(deploy_root: PathBuf, grpc_endpoint: String, db: Option<Arc<DbStore>>) -> Self {
        Self {
            deploy_root,
            grpc_endpoint,
            db,
        }
    }

    /// Current version and process state from gRPC, or DB snapshot if gRPC is down.
    pub async fn get_status(&self) -> Result<StatusView, ControlError> {
        match DeployServiceClient::connect(self.grpc_endpoint.clone()).await {
            Ok(mut client) => match client
                .get_status(tonic::Request::new(StatusRequest {}))
                .await
            {
                Ok(resp) => {
                    let r = resp.into_inner();
                    Ok(StatusView {
                        current_version: r.current_version,
                        state: r.state,
                        source: "grpc",
                    })
                }
                Err(e) => self.status_fallback(e.to_string()).await,
            },
            Err(e) => self.status_fallback(e.to_string()).await,
        }
    }

    async fn status_fallback(&self, err: String) -> Result<StatusView, ControlError> {
        if let Some(db) = &self.db {
            if let Some(row) = db.get_snapshot().await? {
                tracing::warn!(%err, "grpc status failed; using database snapshot");
                return Ok(StatusView {
                    current_version: row.current_version,
                    state: row.state,
                    source: "database",
                });
            }
        }
        Err(ControlError::Grpc(err))
    }

    pub fn list_releases(&self) -> Result<ReleasesView, ControlError> {
        let releases = list_release_versions(&self.deploy_root)?;
        Ok(ReleasesView { releases })
    }

    pub async fn fetch_history(&self, limit: i64) -> Result<HistoryView, ControlError> {
        let Some(db) = &self.db else {
            return Ok(HistoryView { events: vec![] });
        };
        let events = db.fetch_history(limit).await?;
        Ok(HistoryView { events })
    }

    /// One reconcile step: copy gRPC status into `service_snapshot` (used by background task).
    pub async fn reconcile_snapshot(&self, db: &DbStore) -> Result<(), ControlError> {
        let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        let r = client
            .get_status(tonic::Request::new(StatusRequest {}))
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?
            .into_inner();
        db.upsert_snapshot(&r.current_version, &r.state, None)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_releases_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = ControlPlane::new(tmp.path().to_path_buf(), "http://[::1]:9".into(), None);
        let r = p.list_releases().unwrap();
        assert!(r.releases.is_empty());
    }

    #[test]
    fn list_releases_sorts_names() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = tmp.path().join("releases");
        fs::create_dir_all(rel.join("v2")).unwrap();
        fs::create_dir_all(rel.join("v1")).unwrap();
        let p = ControlPlane::new(tmp.path().to_path_buf(), "http://[::1]:9".into(), None);
        let r = p.list_releases().unwrap();
        assert_eq!(r.releases, vec!["v1", "v2"]);
    }
}
