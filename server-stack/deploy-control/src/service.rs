use crate::types::{
    DataSourceItemView, DataSourcesListView, DatabaseColumnsView, DatabaseInfoView,
    DatabaseRelationshipsView, DatabaseSchemasView, DatabaseTablePreviewView, DatabaseTablesView,
    HistoryView, LocalClientConnect, ProjectView, ProjectsView, ReleasesView, StatusView,
};
use deploy_auth::attach_auth_metadata;
use deploy_core::{
    list_release_versions, normalize_project_id, project_deploy_root, validate_project_id,
};
use deploy_db::{
    explorer_columns, explorer_foreign_keys, explorer_schemas, explorer_table_preview,
    explorer_tables, fetch_postgres_server_info, DataSourceRow, DbStore, PgPool,
};
use uuid::Uuid;
use deploy_proto::deploy::deploy_service_client::DeployServiceClient;
use deploy_proto::deploy::{
    CreateConnectionRequest, CreateConnectionResponse, ProxyConnectionPolicy,
    RestartProcessRequest, RollbackRequest, StatusRequest, StopProcessRequest,
};
use ed25519_dalek::SigningKey;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

fn sanitize_config_json_public(v: &serde_json::Value) -> serde_json::Value {
    let mut out = v.clone();
    if let Some(m) = out.as_object_mut() {
        m.remove("password");
        m.remove("passwd");
    }
    out
}

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
    /// Metadata DB (SQLite on native install, PostgreSQL in Docker).
    pub db: Option<Arc<DbStore>>,
    /// Optional PostgreSQL for dashboard schema explorer + `database-info` (same as metadata when that is Postgres, or `POSTGRES_EXPLORER_URL`).
    pub pg_explorer: Option<Arc<PgPool>>,
    /// If set, `GetStatus` gRPC calls include deploy-auth metadata (when deploy-server requires auth).
    pub grpc_signing_key: Option<Arc<SigningKey>>,
}

impl ControlPlane {
    pub fn new(
        deploy_root: PathBuf,
        grpc_endpoint: String,
        db: Option<Arc<DbStore>>,
        pg_explorer: Option<Arc<PgPool>>,
        grpc_signing_key: Option<Arc<SigningKey>>,
    ) -> Self {
        Self {
            deploy_root,
            grpc_endpoint,
            db,
            pg_explorer,
            grpc_signing_key,
        }
    }

    pub fn deploy_root(&self) -> &Path {
        &self.deploy_root
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
                    let local_client = if !r.client_connect_url.is_empty()
                        || !r.client_connect_token.is_empty()
                    {
                        Some(LocalClientConnect {
                            token: r.client_connect_token,
                            url: r.client_connect_url,
                            pairing: r.client_connect_pairing,
                        })
                    } else {
                        None
                    };
                    Ok(StatusView {
                        current_version: r.current_version,
                        state: r.state,
                        source: "grpc",
                        local_client,
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
                    local_client: None,
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

    /// Snapshot of PostgreSQL for the dashboard explorer pool. `connection_display` is a password-redacted URL.
    pub async fn database_info(
        &self,
        connection_display: Option<String>,
    ) -> Result<DatabaseInfoView, ControlError> {
        let Some(pool) = &self.pg_explorer else {
            return Ok(DatabaseInfoView {
                configured: false,
                connection_display: None,
                server_version: None,
                database_name: None,
                session_user: None,
                database_size_bytes: None,
                active_connections: None,
            });
        };
        let row = fetch_postgres_server_info(pool).await?;
        Ok(DatabaseInfoView {
            configured: true,
            connection_display,
            server_version: Some(row.server_version),
            database_name: Some(row.database_name),
            session_user: Some(row.session_user),
            database_size_bytes: Some(row.database_size_bytes),
            active_connections: Some(row.active_connections),
        })
    }

    pub async fn database_schemas(&self) -> Result<DatabaseSchemasView, ControlError> {
        let Some(pool) = &self.pg_explorer else {
            return Ok(DatabaseSchemasView {
                configured: false,
                schemas: vec![],
            });
        };
        let schemas = explorer_schemas(pool).await?;
        Ok(DatabaseSchemasView {
            configured: true,
            schemas,
        })
    }

    pub async fn database_tables(&self, schema: &str) -> Result<DatabaseTablesView, ControlError> {
        let Some(pool) = &self.pg_explorer else {
            return Ok(DatabaseTablesView {
                configured: false,
                schema: schema.to_string(),
                tables: vec![],
            });
        };
        let tables = explorer_tables(pool, schema).await?;
        Ok(DatabaseTablesView {
            configured: true,
            schema: schema.to_string(),
            tables,
        })
    }

    pub async fn database_columns(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<DatabaseColumnsView, ControlError> {
        let Some(pool) = &self.pg_explorer else {
            return Ok(DatabaseColumnsView {
                configured: false,
                schema: schema.to_string(),
                table: table.to_string(),
                columns: vec![],
            });
        };
        let columns = explorer_columns(pool, schema, table).await?;
        Ok(DatabaseColumnsView {
            configured: true,
            schema: schema.to_string(),
            table: table.to_string(),
            columns,
        })
    }

    pub async fn database_relationships(&self) -> Result<DatabaseRelationshipsView, ControlError> {
        let Some(pool) = &self.pg_explorer else {
            return Ok(DatabaseRelationshipsView {
                configured: false,
                foreign_keys: vec![],
            });
        };
        let foreign_keys = explorer_foreign_keys(pool).await?;
        Ok(DatabaseRelationshipsView {
            configured: true,
            foreign_keys,
        })
    }

    pub async fn database_table_preview(
        &self,
        schema: &str,
        table: &str,
        limit: i64,
        offset: i64,
    ) -> Result<DatabaseTablePreviewView, ControlError> {
        let Some(pool) = &self.pg_explorer else {
            return Ok(DatabaseTablePreviewView {
                configured: false,
                schema: schema.to_string(),
                table: table.to_string(),
                preview: None,
            });
        };
        let preview = explorer_table_preview(pool, schema, table, limit, offset).await?;
        Ok(DatabaseTablePreviewView {
            configured: true,
            schema: schema.to_string(),
            table: table.to_string(),
            preview: Some(preview),
        })
    }

    pub async fn data_sources_list(&self) -> Result<DataSourcesListView, ControlError> {
        let mut sources = Vec::new();
        if self.pg_explorer.is_some() {
            sources.push(DataSourceItemView {
                id: "postgresql".to_string(),
                kind: "postgresql".to_string(),
                label: "PostgreSQL".to_string(),
                mount_state: None,
                smb_host: None,
                smb_share: None,
                smb_subpath: None,
                mount_point: None,
                last_error: None,
                config_json: None,
                has_credentials: None,
            });
        }
        let Some(db) = &self.db else {
            return Ok(DataSourcesListView { sources });
        };
        let rows = db.data_sources_list_all().await?;
        for r in rows {
            let has_cred = r.credentials_path.as_ref().map(|p| !p.is_empty()).unwrap_or(false);
            let config_json = r.config_json.as_ref().map(sanitize_config_json_public);
            sources.push(DataSourceItemView {
                id: r.id.to_string(),
                kind: r.kind.clone(),
                label: r.label,
                mount_state: Some(r.mount_state),
                smb_host: r.smb_host.clone(),
                smb_share: r.smb_share.clone(),
                smb_subpath: r.smb_subpath.clone(),
                mount_point: r.mount_point.clone(),
                last_error: r.last_error,
                config_json,
                has_credentials: Some(has_cred),
            });
        }
        Ok(DataSourcesListView { sources })
    }

    pub async fn data_sources_get_smb(&self, id: Uuid) -> Result<Option<DataSourceRow>, ControlError> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        let row = db.data_sources_get(id).await?;
        Ok(row)
    }

    pub async fn data_sources_insert_smb(
        &self,
        id: Uuid,
        label: &str,
        smb_host: &str,
        smb_share: &str,
        smb_subpath: &str,
        mount_point: &str,
        credentials_path: &str,
        mount_state: &str,
        last_error: Option<&str>,
    ) -> Result<(), ControlError> {
        let Some(db) = &self.db else {
            return Err(ControlError::Grpc(
                "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)"
                    .to_string(),
            ));
        };
        db.data_sources_insert_smb(
            id,
            label,
            smb_host,
            smb_share,
            smb_subpath,
            mount_point,
            credentials_path,
            mount_state,
            last_error,
        )
        .await?;
        Ok(())
    }

    pub async fn data_sources_insert_connection(
        &self,
        id: Uuid,
        kind: &str,
        label: &str,
        config_json: &serde_json::Value,
        credentials_path: Option<&str>,
        mount_state: &str,
    ) -> Result<(), ControlError> {
        let Some(db) = &self.db else {
            return Err(ControlError::Grpc(
                "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)"
                    .to_string(),
            ));
        };
        db.data_sources_insert_connection(
            id,
            kind,
            label,
            config_json,
            credentials_path,
            mount_state,
        )
        .await?;
        Ok(())
    }

    pub async fn data_sources_delete_row(&self, id: Uuid) -> Result<bool, ControlError> {
        let Some(db) = &self.db else {
            return Ok(false);
        };
        let n = db.data_sources_delete(id).await?;
        Ok(n > 0)
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

    /// Create a managed proxy session via deploy-server `CreateConnection` (signed gRPC).
    pub async fn create_proxy_invitation(
        &self,
        project_id: String,
        board_label: String,
        policy: ProxyConnectionPolicy,
        recipient_client_pubkey_b64: Option<String>,
    ) -> Result<CreateConnectionResponse, ControlError> {
        validate_project_id(&project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(&project_id);
        let sk = self.grpc_signing_key.as_ref().ok_or_else(|| {
            ControlError::Grpc(
                "grpc signing key is not configured; cannot create proxy tunnel invitations"
                    .to_string(),
            )
        })?;
        let recipient = recipient_client_pubkey_b64
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut req = tonic::Request::new(CreateConnectionRequest {
            project_id: pid.clone(),
            board_label,
            policy: Some(policy),
            recipient_client_pubkey_b64: recipient,
        });
        attach_auth_metadata(&mut req, sk, "CreateConnection", &pid, "")
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        client
            .create_connection(req)
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))
            .map(|r| r.into_inner())
    }

    pub async fn list_proxy_invitations(
        &self,
        limit: i64,
        offset: i64,
        revoked_filter: Option<bool>,
    ) -> Result<Vec<deploy_db::GrpcProxySessionRow>, ControlError> {
        let db = self.db.as_ref().ok_or_else(|| {
            ControlError::Grpc(
                "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)"
                    .to_string(),
            )
        })?;
        db.list_grpc_proxy_sessions_page(limit, offset, revoked_filter)
            .await
            .map_err(Into::into)
    }

    pub async fn revoke_proxy_invitation(&self, session_id: &str) -> Result<u64, ControlError> {
        let db = self.db.as_ref().ok_or_else(|| {
            ControlError::Grpc(
                "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)"
                    .to_string(),
            )
        })?;
        db.revoke_grpc_proxy_session_by_id(session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn fetch_proxy_invitation(
        &self,
        session_id: &str,
    ) -> Result<Option<deploy_db::GrpcProxySessionRow>, ControlError> {
        let db = self.db.as_ref().ok_or_else(|| {
            ControlError::Grpc(
                "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)"
                    .to_string(),
            )
        })?;
        db.fetch_grpc_proxy_session_by_id_only(session_id)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_releases_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = ControlPlane::new(tmp.path().to_path_buf(), "http://[::1]:9".into(), None, None, None);
        let r = p.list_releases("default").unwrap();
        assert!(r.releases.is_empty());
    }

    #[test]
    fn list_releases_sorts_names() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = tmp.path().join("releases");
        fs::create_dir_all(rel.join("v2")).unwrap();
        fs::create_dir_all(rel.join("v1")).unwrap();
        let p = ControlPlane::new(tmp.path().to_path_buf(), "http://[::1]:9".into(), None, None, None);
        let r = p.list_releases("default").unwrap();
        assert_eq!(r.releases, vec!["v1", "v2"]);
    }
}
