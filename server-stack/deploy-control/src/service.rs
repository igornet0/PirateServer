use crate::types::{HistoryView, ProjectView, ProjectsView, ReleasesView, StatusView};
use deploy_auth::attach_auth_metadata;
use deploy_core::{
    list_release_versions, normalize_project_id, project_deploy_root, validate_project_id,
};
use deploy_db::DbStore;
use deploy_proto::deploy::deploy_service_client::DeployServiceClient;
use deploy_proto::deploy::{
    RestartProcessRequest, RollbackRequest, StatusRequest, StopProcessRequest,
};
use ed25519_dalek::SigningKey;
use std::fs;
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
    /// If set, `GetStatus` gRPC calls include deploy-auth metadata (when deploy-server requires auth).
    pub grpc_signing_key: Option<Arc<SigningKey>>,
}

impl ControlPlane {
    pub fn new(
        deploy_root: PathBuf,
        grpc_endpoint: String,
        db: Option<Arc<DbStore>>,
        grpc_signing_key: Option<Arc<SigningKey>>,
    ) -> Self {
        Self {
            deploy_root,
            grpc_endpoint,
            db,
            grpc_signing_key,
        }
    }

    fn status_request(&self, project_id: &str) -> Result<tonic::Request<StatusRequest>, ControlError> {
        let mut req = tonic::Request::new(StatusRequest {
            project_id: project_id.to_string(),
        });
        if let Some(ref sk) = self.grpc_signing_key {
            attach_auth_metadata(&mut req, sk, "GetStatus", project_id, "")
                .map_err(|e| ControlError::Grpc(e.to_string()))?;
        }
        Ok(req)
    }

    /// `project_id` empty means `default`.
    pub async fn get_status(&self, project_id: &str) -> Result<StatusView, ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(project_id);
        match DeployServiceClient::connect(self.grpc_endpoint.clone()).await {
            Ok(mut client) => match client.get_status(self.status_request(&pid)?).await {
                Ok(resp) => {
                    let r = resp.into_inner();
                    Ok(StatusView {
                        current_version: r.current_version,
                        state: r.state,
                        source: "grpc",
                    })
                }
                Err(e) => self.status_fallback(e.to_string(), &pid).await,
            },
            Err(e) => self.status_fallback(e.to_string(), &pid).await,
        }
    }

    async fn status_fallback(&self, err: String, project_id: &str) -> Result<StatusView, ControlError> {
        if let Some(db) = &self.db {
            if let Some(row) = db.get_snapshot(project_id).await? {
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

    pub fn list_releases(&self, project_id: &str) -> Result<ReleasesView, ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let root = project_deploy_root(&self.deploy_root, project_id);
        let releases = list_release_versions(&root)?;
        Ok(ReleasesView { releases })
    }

    /// Lists `default` (legacy root) plus one entry per subdirectory of `projects/`.
    pub fn list_projects(&self) -> ProjectsView {
        let mut projects = vec![ProjectView {
            id: "default".to_string(),
            deploy_root: self.deploy_root.display().to_string(),
        }];
        let projects_dir = self.deploy_root.join("projects");
        if let Ok(rd) = fs::read_dir(&projects_dir) {
            for e in rd.flatten() {
                if let Ok(ft) = e.file_type() {
                    if ft.is_dir() {
                        if let Some(name) = e.file_name().to_str() {
                            if validate_project_id(name).is_ok() && normalize_project_id(name) != "default"
                            {
                                projects.push(ProjectView {
                                    id: name.to_string(),
                                    deploy_root: project_deploy_root(&self.deploy_root, name)
                                        .display()
                                        .to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
        projects.sort_by(|a, b| a.id.cmp(&b.id));
        ProjectsView { projects }
    }

    pub async fn fetch_history(
        &self,
        limit: i64,
        project_id: Option<&str>,
    ) -> Result<HistoryView, ControlError> {
        let Some(db) = &self.db else {
            return Ok(HistoryView { events: vec![] });
        };
        let owned: Option<String> = match project_id {
            None => None,
            Some(p) => {
                let t = p.trim();
                if t.is_empty() {
                    None
                } else if normalize_project_id(t) == "default" {
                    Some("default".to_string())
                } else {
                    validate_project_id(t).map_err(|e| ControlError::Grpc(e.to_string()))?;
                    Some(normalize_project_id(t))
                }
            }
        };
        let filter = owned.as_deref();
        let events = db.fetch_history(limit, filter).await?;
        Ok(HistoryView { events })
    }

    fn rollback_request(
        &self,
        version: &str,
        project_id: &str,
    ) -> Result<tonic::Request<RollbackRequest>, ControlError> {
        let mut req = tonic::Request::new(RollbackRequest {
            version: version.to_string(),
            project_id: project_id.to_string(),
        });
        if let Some(ref sk) = self.grpc_signing_key {
            attach_auth_metadata(&mut req, sk, "Rollback", project_id, version)
                .map_err(|e| ControlError::Grpc(e.to_string()))?;
        }
        Ok(req)
    }

    fn stop_process_request(&self, project_id: &str) -> Result<tonic::Request<StopProcessRequest>, ControlError> {
        let mut req = tonic::Request::new(StopProcessRequest {
            project_id: project_id.to_string(),
        });
        if let Some(ref sk) = self.grpc_signing_key {
            attach_auth_metadata(&mut req, sk, "StopProcess", project_id, "")
                .map_err(|e| ControlError::Grpc(e.to_string()))?;
        }
        Ok(req)
    }

    fn restart_process_request(
        &self,
        project_id: &str,
    ) -> Result<tonic::Request<RestartProcessRequest>, ControlError> {
        let mut req = tonic::Request::new(RestartProcessRequest {
            project_id: project_id.to_string(),
        });
        if let Some(ref sk) = self.grpc_signing_key {
            attach_auth_metadata(&mut req, sk, "RestartProcess", project_id, "")
                .map_err(|e| ControlError::Grpc(e.to_string()))?;
        }
        Ok(req)
    }

    pub async fn rollback(&self, version: String, project_id: String) -> Result<crate::types::RollbackView, ControlError> {
        validate_project_id(&project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(&project_id);
        let v = version.clone();
        let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        let r = client
            .rollback(self.rollback_request(&v, &pid)?)
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?
            .into_inner();
        Ok(crate::types::RollbackView {
            status: r.status,
            active_version: r.active_version,
        })
    }

    pub async fn stop_process(&self, project_id: String) -> Result<crate::types::ProcessControlView, ControlError> {
        validate_project_id(&project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(&project_id);
        let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        let r = client
            .stop_process(self.stop_process_request(&pid)?)
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?
            .into_inner();
        Ok(crate::types::ProcessControlView {
            current_version: r.current_version,
            state: r.state,
        })
    }

    pub async fn restart_process(&self, project_id: String) -> Result<crate::types::ProcessControlView, ControlError> {
        validate_project_id(&project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(&project_id);
        let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        let r = client
            .restart_process(self.restart_process_request(&pid)?)
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?
            .into_inner();
        Ok(crate::types::ProcessControlView {
            current_version: r.current_version,
            state: r.state,
        })
    }

    /// One reconcile step per known project: copy gRPC status into `project_snapshots`.
    pub async fn reconcile_snapshot(&self, db: &DbStore) -> Result<(), ControlError> {
        let projects = self.list_projects().projects;
        for pv in projects {
            let pid = normalize_project_id(&pv.id);
            let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
                .await
                .map_err(|e| ControlError::Grpc(e.to_string()))?;
            let r = client
                .get_status(self.status_request(&pid)?)
                .await
                .map_err(|e| ControlError::Grpc(e.to_string()))?
                .into_inner();
            db.upsert_snapshot(&pid, &r.current_version, &r.state, None)
                .await?;
        }
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
        let p = ControlPlane::new(tmp.path().to_path_buf(), "http://[::1]:9".into(), None, None);
        let r = p.list_releases("default").unwrap();
        assert!(r.releases.is_empty());
    }

    #[test]
    fn list_releases_sorts_names() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = tmp.path().join("releases");
        fs::create_dir_all(rel.join("v2")).unwrap();
        fs::create_dir_all(rel.join("v1")).unwrap();
        let p = ControlPlane::new(tmp.path().to_path_buf(), "http://[::1]:9".into(), None, None);
        let r = p.list_releases("default").unwrap();
        assert_eq!(r.releases, vec!["v1", "v2"]);
    }
}
