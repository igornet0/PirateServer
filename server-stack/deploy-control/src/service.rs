use crate::db_admin::{
    migration_run_approved, mysql_create_database, mysql_create_table, postgres_create_connection_user,
    postgres_create_database_for_caller, postgres_create_table, postgres_delete_connection_user,
};
use crate::db_migration::host_migration_status;
use crate::HostDbMigrationStatusView;
use crate::types::{
    DataSourceItemView, DataSourcesListView, DatabaseColumnsView, DatabaseInfoView,
    DatabaseRelationshipsView, DatabaseSchemasView, DatabaseTablePreviewView, DatabaseTablesView,
    HistoryView, HostDatabaseQueryResultView, HostDatabasesListView, HostDbAdminCreateTableBody,
    HostDbAdminCreateUserBody, HostDbAdminCreateUserView, HostDbAdminDeleteUserBody, HostDbAdminDeleteUserView,
    HostDbCreateDatabaseBody, HostDbMigrationRunBody,
    HostDbMigrationRunView, HostDbRequestCredentials,
    HostDeployEnvPutView, HostDeployEnvView, HostDbV2GridBody, HostDbV2GridView, HostDbV2MutationResultView,
    HostDbV2ObjectTreeView, HostDbV2RowMutationBody, HostDbV2SchemaNode, LocalClientConnect,
    ProjectNginxSnippetView, ProjectTelemetryLogLine, ProjectTelemetryView, ProjectView, ProjectsView,
    HostDatabaseQueryBody, HostDatabaseRedisKeysView, ReleasesView, StatusView,
};
use deploy_auth::attach_auth_metadata;
use deploy_core::{
    list_release_versions, nginx_snippet, normalize_project_id, process_manager,
    project_deploy_root, read_current_version_from_symlink, release_dir_for_version,
    validate_project_id,
};
use deploy_core::pirate_project::PirateManifest;
use crate::db_host::{
    clickhouse_base_url_with_creds, clickhouse_query, clickhouse_query_http, connect_ephemeral_postgres,
    list_instances, mongo_collections, mongo_collections_for_url, mongo_databases, mongo_databases_for_url,
    mongo_preview, mongo_preview_for_url, mongodb_url_with_creds, mysql_columns, mysql_columns_for_url, mysql_query,
    mysql_explorer_url_from_env, mysql_query_for_url, mysql_preview, mysql_preview_for_url, mysql_row_mutate,
    mysql_schemas, mysql_schemas_for_url, mysql_tables, mysql_tables_for_url, mysql_url_with_creds, mysql_v2_grid,
    parse_instance_id, pg_columns_view,
    pg_preview, pg_query, pg_rel_view, pg_row_mutate, pg_schemas_view, pg_tables_view, pg_v2_grid, postgres_url_with_creds,
    redis_keys, redis_keys_for_url, redis_url_with_creds, DbHostError,
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
    UpdateProxySettingsRequest, UpdateProxySettingsResponse,
};
use ed25519_dalek::SigningKey;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use thiserror::Error;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

fn sanitize_config_json_public(v: &serde_json::Value) -> serde_json::Value {
    let mut out = v.clone();
    if let Some(m) = out.as_object_mut() {
        m.remove("password");
        m.remove("passwd");
    }
    out
}

fn tail_file_lines(path: &Path, limit: usize) -> std::io::Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)?;
    let mut lines: Vec<String> = raw.lines().map(|s| s.to_string()).collect();
    if lines.len() > limit {
        lines = lines.split_off(lines.len() - limit);
    }
    Ok(lines)
}

const MAX_PROJECT_NGINX_SNIPPET: usize = 512 * 1024;

fn nginx_snippet_state_fields(project_root: &Path) -> (Option<String>, Option<String>, Option<bool>, Option<i64>, Option<String>) {
    let state = deploy_core::nginx_vhost_state::read_nginx_vhost_state(project_root)
        .unwrap_or_default();
    let has_state = !state.template_sha256.is_empty() || !state.applied_content_sha256.is_empty();
    if !has_state {
        return (None, None, None, None, None);
    }
    (
        Some(state.template_sha256).filter(|s| !s.is_empty()),
        Some(state.applied_content_sha256).filter(|s| !s.is_empty()),
        None,
        Some(state.applied_at_ms).filter(|ms| *ms > 0),
        Some(state.applied_site_path).filter(|s| !s.is_empty()),
    )
}

/// `releases/<ver>/pirate-nginx-snippet.conf` after deploy when the manifest requests a nginx release snippet.
fn project_nginx_snippet_view(project_root: &Path, current_version: &str) -> ProjectNginxSnippetView {
    let mut ver = current_version.trim().to_string();
    if ver.is_empty() {
        ver = read_current_version_from_symlink(project_root).unwrap_or_default();
    }
    let ver = ver.trim();
    let placeholder = project_root
        .join("releases")
        .join("<version>")
        .join("pirate-nginx-snippet.conf");
    if ver.is_empty() {
        let (template_sha256, applied_content_sha256, needs_update, last_applied_at_ms, applied_site_path) =
            nginx_snippet_state_fields(project_root);
        return ProjectNginxSnippetView {
            path: placeholder.to_string_lossy().to_string(),
            configured: false,
            status: Some("no_release".to_string()),
            reason_code: Some("no_active_version".to_string()),
            hint: Some(
                "No active release version (current symlink / status empty). Deploy once first."
                    .to_string(),
            ),
            content: None,
            template_sha256,
            applied_content_sha256,
            needs_update,
            last_applied_at_ms,
            applied_site_path,
        };
    }
    let release_dir = release_dir_for_version(project_root, ver);
    let snippet_path = release_dir.join("pirate-nginx-snippet.conf");
    let manifest_path = release_dir.join("pirate.toml");
    let path_str = snippet_path.to_string_lossy().to_string();
    let manifest_opt = PirateManifest::read_file(&manifest_path).ok();

    let absent = |reason_code: &str, hint: String| {
        let (template_sha256, applied_content_sha256, needs_update, last_applied_at_ms, applied_site_path) =
            nginx_snippet_state_fields(project_root);
        ProjectNginxSnippetView {
            path: path_str.clone(),
            configured: false,
            status: Some("absent".to_string()),
            reason_code: Some(reason_code.to_string()),
            hint: Some(hint),
            content: None,
            template_sha256,
            applied_content_sha256,
            needs_update,
            last_applied_at_ms,
            applied_site_path,
        }
    };

    match fs::read_to_string(&snippet_path) {
        Ok(s) if !s.trim().is_empty() => {
            let content = if s.len() > MAX_PROJECT_NGINX_SNIPPET {
                Some(format!(
                    "{}\n\n... [truncated, total {} bytes]",
                    &s[..MAX_PROJECT_NGINX_SNIPPET],
                    s.len()
                ))
            } else {
                Some(s)
            };
            let (template_sha256, applied_content_sha256, needs_update, last_applied_at_ms, applied_site_path) =
                nginx_snippet_state_fields(project_root);
            ProjectNginxSnippetView {
                path: path_str,
                configured: true,
                status: Some("present".to_string()),
                reason_code: None,
                hint: None,
                content,
                template_sha256,
                applied_content_sha256,
                needs_update,
                last_applied_at_ms,
                applied_site_path,
            }
        }
        _ => {
            if let Some(ref m) = manifest_opt {
                if m.proxy.has_custom_nginx_template() {
                    return absent(
                        "custom_template_missing",
                        "Custom nginx template configured but snippet file is missing; redeploy."
                            .to_string(),
                    );
                }
                if let Some(skip) = nginx_snippet::nginx_release_skip(m) {
                    return absent(skip.reason_code(), skip.hint_en().to_string());
                }
                return absent(
                    "not_generated",
                    "Manifest expects a snippet but the file is missing; redeploy or check deploy-server logs."
                        .to_string(),
                );
            }
            absent(
                "no_manifest_in_release",
                "pirate.toml not found under this release; redeploy with a valid manifest."
                    .to_string(),
            )
        }
    }
}

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("grpc: {0}")]
    Grpc(String),
    #[error("host_deploy_env: {0}")]
    HostDeployEnv(String),
    #[error("nginx: {0}")]
    NginxOp(String),
    #[error("host_service: {0}")]
    HostServiceOp(String),
    #[error("antiddos: {0}")]
    Antiddos(String),
    #[error("process_listeners: {0}")]
    ProcessListeners(String),
    #[error("elevation_required: {0}")]
    ElevationRequired(String),
    #[error("elevation_failed: {0}")]
    ElevationFailed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] deploy_db::DbError),
    #[error("host_db: {0}")]
    HostDb(String),
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
    /// `CONTROL_API_HOST_DEPLOY_ENV_PATH` — host services env for DB port discovery and explorer URLs.
    pub host_env_path: std::path::PathBuf,
    /// Host + port of `POSTGRES_EXPLORER_URL` to match a discovered `postgresql` instance.
    pub pg_explorer_target: Option<(String, u16)>,
}

impl ControlPlane {
    pub fn new(
        deploy_root: PathBuf,
        grpc_endpoint: String,
        db: Option<Arc<DbStore>>,
        pg_explorer: Option<Arc<PgPool>>,
        grpc_signing_key: Option<Arc<SigningKey>>,
        host_env_path: std::path::PathBuf,
        pg_explorer_target: Option<(String, u16)>,
    ) -> Self {
        Self {
            deploy_root,
            grpc_endpoint,
            db,
            pg_explorer,
            grpc_signing_key,
            host_env_path,
            pg_explorer_target,
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
                        max_upload_bytes: (r.max_upload_bytes > 0).then_some(r.max_upload_bytes),
                    })
                }
                Err(e) => self.status_fallback(e.to_string(), &pid).await,
            },
            Err(e) => self.status_fallback(e.to_string(), &pid).await,
        }
    }

    pub async fn project_telemetry(
        &self,
        project_id: &str,
        logs_limit: usize,
    ) -> Result<ProjectTelemetryView, ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(project_id);
        let root = project_deploy_root(&self.deploy_root, &pid);
        let status = self.get_status(&pid).await?;

        let runtime = process_manager::read_runtime_state(&root);
        let process_pid = runtime.as_ref().and_then(|r| r.pid);

        let mut cpu_percent = None;
        let mut ram_used_bytes = None;
        let mut ram_percent = None;
        let gpu_percent = None;
        let mut telemetry_available = false;

        if let Some(ppid) = process_pid {
            let mut sys = System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
            );
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[Pid::from_u32(ppid)]),
                true,
                ProcessRefreshKind::everything(),
            );
            if let Some(proc_item) = sys.process(Pid::from_u32(ppid)) {
                cpu_percent = Some(proc_item.cpu_usage());
                let mem = proc_item.memory();
                ram_used_bytes = Some(mem);
                let total = sys.total_memory();
                if total > 0 {
                    ram_percent = Some((mem as f32 / total as f32) * 100.0);
                }
                telemetry_available = true;
            }
        }

        let log_path = root.join(".pirate").join("runtime.log");
        let mut logs_available = false;
        let mut logs_tail = Vec::new();
        if log_path.is_file() {
            let tail = tail_file_lines(&log_path, logs_limit.max(1)).unwrap_or_default();
            logs_tail = tail
                .into_iter()
                .map(|message| ProjectTelemetryLogLine {
                    ts_ms: chrono::Utc::now().timestamp_millis(),
                    level: "info".to_string(),
                    message,
                })
                .collect();
            logs_available = true;
        }

        let project_nginx = project_nginx_snippet_view(&root, &status.current_version);

        Ok(ProjectTelemetryView {
            project_id: pid,
            state: status.state,
            pid: process_pid,
            cpu_percent,
            ram_used_bytes,
            ram_percent,
            gpu_percent,
            telemetry_available,
            logs_available,
            logs_tail,
            collected_at_ms: chrono::Utc::now().timestamp_millis(),
            project_nginx,
        })
    }

    /// Truncate `<deploy_root>/.pirate/runtime.log` for the project (creates empty file if missing).
    pub async fn clear_project_runtime_log(&self, project_id: &str) -> Result<(), ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(project_id);
        let root = project_deploy_root(&self.deploy_root, &pid);
        let log_path = root.join(".pirate").join("runtime.log");
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&log_path, b"").await?;
        Ok(())
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
                    max_upload_bytes: None,
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

    /// Create a new non-`default` project id: directory under `projects/` plus optional DB row.
    /// Safe to call before first deploy; id is recorded in `project_snapshots` when DB is configured.
    pub async fn allocate_project_id(&self) -> Result<crate::types::AllocateProjectResponse, ControlError> {
        for _attempt in 0..96u32 {
            let id = format!("p-{}", Uuid::new_v4().as_simple());
            validate_project_id(&id).map_err(|e| ControlError::Grpc(e.to_string()))?;
            let root = project_deploy_root(&self.deploy_root, &id);
            if root.exists() {
                continue;
            }
            fs::create_dir_all(&root)?;
            if let Some(db) = &self.db {
                if let Err(e) = db
                    .upsert_snapshot(&id, "", "stopped", None)
                    .await
                {
                    let _ = fs::remove_dir_all(&root);
                    return Err(e.into());
                }
            }
            return Ok(crate::types::AllocateProjectResponse { id });
        }
        Err(ControlError::Grpc(
            "could not allocate a unique project id".into(),
        ))
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

    pub fn list_process_listeners(
        &self,
        project_id: &str,
        scope: &str,
    ) -> Result<crate::types::ProcessListenersView, ControlError> {
        crate::process_listeners::list_process_listeners(&self.deploy_root, project_id, scope)
    }

    pub fn kill_process_listener(
        &self,
        project_id: &str,
        pid: u32,
        signal: &str,
        port: Option<u16>,
        root_password: Option<&str>,
        allow_foreign: bool,
    ) -> Result<crate::types::KillListenerResultView, ControlError> {
        crate::process_listeners::kill_process_listener(
            &self.deploy_root,
            project_id,
            pid,
            signal,
            port,
            root_password,
            allow_foreign,
        )
    }

    /// `app.env` in the project deploy root (same layout as `releases/`).
    pub fn read_app_env(&self, project_id: &str) -> Result<crate::types::AppEnvView, ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(project_id);
        let path = project_deploy_root(&self.deploy_root, &pid).join("app.env");
        let exists = path.is_file();
        let content = if exists {
            fs::read_to_string(&path)?
        } else {
            String::new()
        };
        Ok(crate::types::AppEnvView {
            path: path.to_string_lossy().to_string(),
            content,
            exists,
        })
    }

    pub fn write_app_env(&self, project_id: &str, content: &str) -> Result<crate::types::AppEnvView, ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(project_id);
        let path = project_deploy_root(&self.deploy_root, &pid).join("app.env");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content.as_bytes())?;
        Ok(crate::types::AppEnvView {
            path: path.to_string_lossy().to_string(),
            content: content.to_string(),
            exists: true,
        })
    }

    /// Max size for `/etc/pirate-deploy.env` via control-api (matches helper script).
    pub const MAX_HOST_DEPLOY_ENV_BYTES: usize = 512 * 1024;

    /// Read host env file (same path systemd `EnvironmentFile` uses for deploy-server / control-api).
    pub fn read_host_deploy_env(path: &Path) -> Result<HostDeployEnvView, ControlError> {
        let exists = path.is_file();
        let content = if exists {
            fs::read_to_string(path)?
        } else {
            String::new()
        };
        Ok(HostDeployEnvView {
            path: path.to_string_lossy().to_string(),
            content,
            exists,
        })
    }

    /// Write host env via `sudo -n pirate-write-deploy-env.sh <path>` (stdin = new content).
    pub fn write_host_deploy_env(
        path: &Path,
        content: &str,
        helper_script: &Path,
    ) -> Result<HostDeployEnvPutView, ControlError> {
        if content.len() > Self::MAX_HOST_DEPLOY_ENV_BYTES {
            return Err(ControlError::HostDeployEnv(format!(
                "content exceeds {} bytes",
                Self::MAX_HOST_DEPLOY_ENV_BYTES
            )));
        }
        if content.as_bytes().contains(&0) {
            return Err(ControlError::HostDeployEnv(
                "content must not contain NUL bytes".into(),
            ));
        }
        let Some(target) = path.to_str() else {
            return Err(ControlError::HostDeployEnv("invalid UTF-8 path".into()));
        };
        if helper_script.is_file() {
            let mut child = Command::new("sudo")
                .args([
                    "-n",
                    helper_script
                        .to_str()
                        .ok_or_else(|| ControlError::HostDeployEnv("invalid helper path".into()))?,
                    target,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| ControlError::HostDeployEnv(format!("sudo spawn: {e}")))?;
            let mut stdin = child.stdin.take().ok_or_else(|| {
                ControlError::HostDeployEnv("sudo: stdin not available".into())
            })?;
            std::io::Write::write_all(&mut stdin, content.as_bytes())
                .map_err(|e| ControlError::HostDeployEnv(format!("sudo stdin: {e}")))?;
            drop(stdin);
            let out = child
                .wait_with_output()
                .map_err(|e| ControlError::HostDeployEnv(format!("sudo wait: {e}")))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stdout = String::from_utf8_lossy(&out.stdout);
                return Err(ControlError::HostDeployEnv(format!(
                    "helper failed (status {}): {} {}",
                    out.status,
                    stdout.trim(),
                    stderr.trim()
                )));
            }
            let restart_scheduled = String::from_utf8_lossy(&out.stdout)
                .to_string()
                .contains("restart shortly")
                || String::from_utf8_lossy(&out.stderr).contains("restart");
            return Ok(HostDeployEnvPutView {
                path: path.to_string_lossy().to_string(),
                content: content.to_string(),
                exists: true,
                restart_scheduled,
            });
        }

        fs::write(path, content.as_bytes())?;
        Ok(HostDeployEnvPutView {
            path: path.to_string_lossy().to_string(),
            content: content.to_string(),
            exists: true,
            restart_scheduled: false,
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

    fn map_host_db(e: DbHostError) -> ControlError {
        ControlError::HostDb(e.to_string())
    }

    /// Pool when the instance is PostgreSQL and matches `POSTGRES_EXPLORER_URL` host:port.
    fn pg_pool_for_host_instance(&self, instance_id: &str) -> Option<Arc<PgPool>> {
        let (engine, h, p) = parse_instance_id(instance_id)?;
        if engine != "postgresql" {
            return None;
        }
        let t = self.pg_explorer_target.as_ref()?;
        if t.0 == h && t.1 == p {
            return self.pg_explorer.clone();
        }
        None
    }

    pub async fn host_databases_list(&self) -> Result<HostDatabasesListView, ControlError> {
        Ok(
            list_instances(&self.host_env_path, self.pg_explorer_target.as_ref())
                .await,
        )
    }

    pub async fn host_db_schemas_json(
        &self,
        instance_id: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<serde_json::Value, ControlError> {
        if let Some(c) = creds {
            let (engine, _, _) = parse_instance_id(instance_id)
                .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
            if engine == "postgresql" {
                let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let pool = connect_ephemeral_postgres(&url)
                    .await
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let v = pg_schemas_view(&pool)
                    .await
                    .map_err(Self::map_host_db)?;
                return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
            }
            if engine == "mysql" {
                let url = mysql_url_with_creds(instance_id, &c.user, &c.pass)
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let r = mysql_schemas_for_url(&url)
                    .await
                    .map_err(Self::map_host_db)?;
                return Ok(serde_json::json!({
                    "engine": "mysql",
                    "configured": true,
                    "schemas": r
                }));
            }
        }
        if let Some(ref pool) = self.pg_pool_for_host_instance(instance_id) {
            let v = pg_schemas_view(pool)
                .await
                .map_err(Self::map_host_db)?;
            return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
        }
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        if engine == "mysql" {
            let r = mysql_schemas(&self.host_env_path)
                .await
                .map_err(Self::map_host_db)?;
            return Ok(serde_json::json!({
                "engine": "mysql",
                "configured": true,
                "schemas": r
            }));
        }
        Err(ControlError::HostDb(
            "schemas not available for this instance (set explorer URL / check POSTGRES_EXPLORER_URL host:port match)".into(),
        ))
    }

    pub async fn host_db_tables_json(
        &self,
        instance_id: &str,
        schema: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<serde_json::Value, ControlError> {
        if let Some(c) = creds {
            let (engine, _, _) = parse_instance_id(instance_id)
                .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
            if engine == "postgresql" {
                let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let pool = connect_ephemeral_postgres(&url)
                    .await
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let v = pg_tables_view(&pool, schema)
                    .await
                    .map_err(Self::map_host_db)?;
                return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
            }
            if engine == "mysql" {
                let url = mysql_url_with_creds(instance_id, &c.user, &c.pass)
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let r = mysql_tables_for_url(&url, schema)
                    .await
                    .map_err(Self::map_host_db)?;
                return Ok(serde_json::json!({ "engine": "mysql", "tables": r }));
            }
        }
        if let Some(ref pool) = self.pg_pool_for_host_instance(instance_id) {
            let v = pg_tables_view(pool, schema)
                .await
                .map_err(Self::map_host_db)?;
            return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
        }
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        if engine == "mysql" {
            let r = mysql_tables(&self.host_env_path, schema)
                .await
                .map_err(Self::map_host_db)?;
            return Ok(serde_json::json!({ "engine": "mysql", "tables": r }));
        }
        Err(ControlError::HostDb("tables not available".into()))
    }

    pub async fn host_db_columns_json(
        &self,
        instance_id: &str,
        schema: &str,
        table: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<serde_json::Value, ControlError> {
        if let Some(c) = creds {
            let (engine, _, _) = parse_instance_id(instance_id)
                .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
            if engine == "postgresql" {
                let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let pool = connect_ephemeral_postgres(&url)
                    .await
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let v = pg_columns_view(&pool, schema, table)
                    .await
                    .map_err(Self::map_host_db)?;
                return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
            }
            if engine == "mysql" {
                let url = mysql_url_with_creds(instance_id, &c.user, &c.pass)
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let r = mysql_columns_for_url(&url, schema, table)
                    .await
                    .map_err(Self::map_host_db)?;
                return Ok(serde_json::json!({ "engine": "mysql", "columns": r }));
            }
        }
        if let Some(ref pool) = self.pg_pool_for_host_instance(instance_id) {
            let v = pg_columns_view(pool, schema, table)
                .await
                .map_err(Self::map_host_db)?;
            return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
        }
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        if engine == "mysql" {
            let r = mysql_columns(&self.host_env_path, schema, table)
                .await
                .map_err(Self::map_host_db)?;
            return Ok(serde_json::json!({ "engine": "mysql", "columns": r }));
        }
        Err(ControlError::HostDb("columns not available".into()))
    }

    pub async fn host_db_rows_json(
        &self,
        instance_id: &str,
        schema: &str,
        table: &str,
        limit: u32,
        offset: u32,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<serde_json::Value, ControlError> {
        if let Some(c) = creds {
            let (engine, _, _) = parse_instance_id(instance_id)
                .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
            if engine == "postgresql" {
                let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let pool = connect_ephemeral_postgres(&url)
                    .await
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let v = pg_preview(&pool, schema, table, i64::from(limit), i64::from(offset))
                    .await
                    .map_err(Self::map_host_db)?;
                return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
            }
            if engine == "mysql" {
                let url = mysql_url_with_creds(instance_id, &c.user, &c.pass)
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let v = mysql_preview_for_url(&url, schema, table, limit, offset)
                    .await
                    .map_err(Self::map_host_db)?;
                return Ok(serde_json::json!({ "engine": "mysql", "preview": v }));
            }
        }
        if let Some(ref pool) = self.pg_pool_for_host_instance(instance_id) {
            let v = pg_preview(pool, schema, table, i64::from(limit), i64::from(offset))
                .await
                .map_err(Self::map_host_db)?;
            return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
        }
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        if engine == "mysql" {
            let v = mysql_preview(
                &self.host_env_path,
                schema,
                table,
                limit,
                offset,
            )
            .await
            .map_err(Self::map_host_db)?;
            return Ok(serde_json::json!({ "engine": "mysql", "preview": v }));
        }
        Err(ControlError::HostDb("row preview not available for this engine".into()))
    }

    pub async fn host_db_relationships_json(
        &self,
        instance_id: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<serde_json::Value, ControlError> {
        if let Some(c) = creds {
            let (engine, _, _) = parse_instance_id(instance_id)
                .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
            if engine == "postgresql" {
                let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let pool = connect_ephemeral_postgres(&url)
                    .await
                    .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                let v = pg_rel_view(&pool)
                    .await
                    .map_err(Self::map_host_db)?;
                return serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()));
            }
        }
        let pool = self
            .pg_pool_for_host_instance(instance_id)
            .ok_or_else(|| {
                ControlError::HostDb(
                    "foreign keys are only for PostgreSQL (set POSTGRES_EXPLORER_URL or pass X-Pirate-Db-* headers)"
                        .into(),
                )
            })?;
        let v = pg_rel_view(&pool)
            .await
            .map_err(Self::map_host_db)?;
        serde_json::to_value(v).map_err(|e| ControlError::HostDb(e.to_string()))
    }

    pub async fn host_db_query(
        &self,
        instance_id: &str,
        body: &HostDatabaseQueryBody,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<HostDatabaseQueryResultView, ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        match engine.as_str() {
            "postgresql" => {
                if let Some(c) = creds {
                    let db = body
                        .database
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("postgres");
                    deploy_db::validate_pg_ident(db).map_err(|e: deploy_db::DbError| {
                        ControlError::HostDb(e.to_string())
                    })?;
                    let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, db)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    let pool = connect_ephemeral_postgres(&url)
                        .await
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    return pg_query(&pool, &body.sql, body.max_rows)
                        .await
                        .map_err(Self::map_host_db);
                }
                let pool = self
                    .pg_pool_for_host_instance(instance_id)
                    .ok_or_else(|| {
                        ControlError::HostDb(
                            "POSTGRES_EXPLORER_URL host:port does not match this instance"
                                .into(),
                        )
                    })?;
                pg_query(&pool, &body.sql, body.max_rows)
                    .await
                    .map_err(Self::map_host_db)
            }
            "mysql" => {
                if let Some(c) = creds {
                    let mut url_str = mysql_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    if let Some(ref d) = body.database {
                        let d = d.trim();
                        if !d.is_empty() {
                            if d.len() > 64
                                || !d.chars().all(|ch| {
                                    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '$'
                                })
                            {
                                return Err(ControlError::HostDb(
                                    "invalid MySQL database name".into(),
                                ));
                            }
                            let mut u = url::Url::parse(&url_str).map_err(|e| {
                                ControlError::HostDb(format!("mysql URL: {e}"))
                            })?;
                            u.set_path(&format!("/{d}"));
                            url_str = u.to_string();
                        }
                    }
                    return mysql_query_for_url(&url_str, &body.sql, body.max_rows)
                        .await
                        .map_err(Self::map_host_db);
                }
                mysql_query(&self.host_env_path, instance_id, &body.sql, body.max_rows)
                    .await
                    .map_err(Self::map_host_db)
            }
            "clickhouse" => {
                if let Some(c) = creds {
                    let u = clickhouse_base_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    return clickhouse_query_http(&u, &body.sql, body.max_rows)
                        .await
                        .map_err(Self::map_host_db);
                }
                clickhouse_query(&self.host_env_path, instance_id, &body.sql, body.max_rows)
                    .await
                    .map_err(Self::map_host_db)
            }
            _ => Err(ControlError::HostDb("SQL query not supported for this engine".into())),
        }
    }

    pub async fn host_db_redis_keys(
        &self,
        instance_id: &str,
        pattern: &str,
        cursor: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<HostDatabaseRedisKeysView, ControlError> {
        if let Some(c) = creds {
            let url = redis_url_with_creds(instance_id, &c.user, &c.pass)
                .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
            return redis_keys_for_url(&url, pattern, cursor)
                .await
                .map_err(Self::map_host_db);
        }
        redis_keys(&self.host_env_path, instance_id, pattern, cursor)
            .await
            .map_err(Self::map_host_db)
    }

    pub async fn host_db_mongo_dbs(
        &self,
        instance_id: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<Vec<String>, ControlError> {
        let (e, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid id".into()))?;
        if e != "mongodb" {
            return Err(ControlError::HostDb("not a MongoDB instance id".into()));
        }
        if let Some(c) = creds {
            let url = mongodb_url_with_creds(instance_id, &c.user, &c.pass)
                .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
            return mongo_databases_for_url(&url)
                .await
                .map_err(Self::map_host_db);
        }
        mongo_databases(&self.host_env_path)
            .await
            .map_err(Self::map_host_db)
    }

    pub async fn host_db_mongo_collections(
        &self,
        instance_id: &str,
        db_name: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<Vec<String>, ControlError> {
        let (e, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid id".into()))?;
        if e != "mongodb" {
            return Err(ControlError::HostDb("not a MongoDB instance id".into()));
        }
        if let Some(c) = creds {
            let url = mongodb_url_with_creds(instance_id, &c.user, &c.pass)
                .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
            return mongo_collections_for_url(&url, db_name)
                .await
                .map_err(Self::map_host_db);
        }
        mongo_collections(&self.host_env_path, db_name)
            .await
            .map_err(Self::map_host_db)
    }

    pub async fn host_db_mongo_preview(
        &self,
        instance_id: &str,
        db_name: &str,
        collection: &str,
        limit: u32,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<Vec<serde_json::Value>, ControlError> {
        let (e, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid id".into()))?;
        if e != "mongodb" {
            return Err(ControlError::HostDb("not a MongoDB instance id".into()));
        }
        if let Some(c) = creds {
            let url = mongodb_url_with_creds(instance_id, &c.user, &c.pass)
                .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
            return mongo_preview_for_url(&url, db_name, collection, limit)
                .await
                .map_err(Self::map_host_db);
        }
        mongo_preview(
            &self.host_env_path,
            db_name,
            collection,
            limit,
        )
        .await
        .map_err(Self::map_host_db)
    }

    /// DBeaver-like object tree (schemas → tables) for supported engines.
    pub async fn host_db_v2_object_tree(
        &self,
        instance_id: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<HostDbV2ObjectTreeView, ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        match engine.as_str() {
            "postgresql" => {
                let pool: Arc<PgPool> = if let Some(c) = creds {
                    let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    Arc::new(
                        connect_ephemeral_postgres(&url)
                            .await
                            .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?,
                    )
                } else {
                    self.pg_pool_for_host_instance(instance_id)
                        .ok_or_else(|| {
                            ControlError::HostDb(
                                "PostgreSQL tree: set POSTGRES_EXPLORER_URL match or pass X-Pirate-Db-*"
                                    .into(),
                            )
                        })?
                };
                let sv = pg_schemas_view(&*pool)
                    .await
                    .map_err(Self::map_host_db)?;
                let mut schemas: Vec<HostDbV2SchemaNode> = Vec::new();
                for s in sv.schemas.into_iter().take(200) {
                    let tv = pg_tables_view(&*pool, &s.name)
                        .await
                        .map_err(Self::map_host_db)?;
                    let tables: Vec<String> = tv.tables.into_iter().map(|t| t.name).take(500).collect();
                    schemas.push(HostDbV2SchemaNode {
                        name: s.name,
                        tables,
                    });
                }
                Ok(HostDbV2ObjectTreeView {
                    version: 2,
                    engine,
                    schemas,
                })
            }
            "mysql" => {
                let mut schemas: Vec<HostDbV2SchemaNode> = Vec::new();
                if let Some(c) = creds {
                    let url = mysql_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    let sch_rows = mysql_schemas_for_url(&url)
                        .await
                        .map_err(Self::map_host_db)?;
                    let names: Vec<String> = sch_rows
                        .into_iter()
                        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .take(200)
                        .collect();
                    for name in names {
                        let tr = mysql_tables_for_url(&url, &name)
                            .await
                            .map_err(Self::map_host_db)?;
                        let tables: Vec<String> = tr
                            .iter()
                            .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                            .take(500)
                            .collect();
                        schemas.push(HostDbV2SchemaNode { name, tables });
                    }
                } else {
                    let sch_rows = mysql_schemas(&self.host_env_path)
                        .await
                        .map_err(Self::map_host_db)?;
                    let names: Vec<String> = sch_rows
                        .into_iter()
                        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .take(200)
                        .collect();
                    for name in names {
                        let tr = mysql_tables(&self.host_env_path, &name)
                            .await
                            .map_err(Self::map_host_db)?;
                        let tables: Vec<String> = tr
                            .iter()
                            .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                            .take(500)
                            .collect();
                        schemas.push(HostDbV2SchemaNode { name, tables });
                    }
                }
                Ok(HostDbV2ObjectTreeView {
                    version: 2,
                    engine,
                    schemas,
                })
            }
            "clickhouse" => {
                let sql = "SELECT name FROM system.tables WHERE database = currentDatabase() ORDER BY name";
                let res = if let Some(c) = creds {
                    let u = clickhouse_base_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    clickhouse_query_http(&u, sql, 2000)
                        .await
                        .map_err(Self::map_host_db)?
                } else {
                    clickhouse_query(&self.host_env_path, instance_id, sql, 2000)
                        .await
                        .map_err(Self::map_host_db)?
                };
                let tables: Vec<String> = res
                    .rows
                    .iter()
                    .filter_map(|r| {
                        r.as_object()
                            .and_then(|o| o.get("name").or_else(|| o.get("NAME")))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect();
                Ok(HostDbV2ObjectTreeView {
                    version: 2,
                    engine,
                    schemas: vec![HostDbV2SchemaNode {
                        name: "default".into(),
                        tables,
                    }],
                })
            }
            "redis" => Ok(HostDbV2ObjectTreeView {
                version: 2,
                engine,
                schemas: vec![HostDbV2SchemaNode {
                    name: "keyspace".into(),
                    tables: vec!["(keys)".into()],
                }],
            }),
            "mongodb" => {
                if let Some(c) = creds {
                    let url = mongodb_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    let dbs = mongo_databases_for_url(&url)
                        .await
                        .map_err(Self::map_host_db)?;
                    let mut schemas: Vec<HostDbV2SchemaNode> = Vec::new();
                    for dbn in dbs.into_iter().take(100) {
                        let colls = mongo_collections_for_url(&url, &dbn)
                            .await
                            .map_err(Self::map_host_db)?;
                        schemas.push(HostDbV2SchemaNode {
                            name: dbn,
                            tables: colls.into_iter().take(500).collect(),
                        });
                    }
                    Ok(HostDbV2ObjectTreeView {
                        version: 2,
                        engine,
                        schemas,
                    })
                } else {
                    let dbs = mongo_databases(&self.host_env_path)
                        .await
                        .map_err(Self::map_host_db)?;
                    let mut schemas: Vec<HostDbV2SchemaNode> = Vec::new();
                    for dbn in dbs.into_iter().take(100) {
                        let colls = mongo_collections(&self.host_env_path, &dbn)
                            .await
                            .map_err(Self::map_host_db)?;
                        schemas.push(HostDbV2SchemaNode {
                            name: dbn,
                            tables: colls.into_iter().take(500).collect(),
                        });
                    }
                    Ok(HostDbV2ObjectTreeView {
                        version: 2,
                        engine,
                        schemas,
                    })
                }
            }
            _ => Err(ControlError::HostDb(
                "object tree not available for this engine".into(),
            )),
        }
    }

    /// Filtered / sorted data grid (PostgreSQL and MySQL).
    pub async fn host_db_v2_grid(
        &self,
        instance_id: &str,
        body: &HostDbV2GridBody,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<HostDbV2GridView, ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        let limit = body.limit;
        let offset = body.offset;
        match engine.as_str() {
            "postgresql" => {
                let pool: Arc<PgPool> = if let Some(c) = creds {
                    let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    Arc::new(
                        connect_ephemeral_postgres(&url)
                            .await
                            .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?,
                    )
                } else {
                    self.pg_pool_for_host_instance(instance_id)
                        .ok_or_else(|| {
                            ControlError::HostDb("PostgreSQL grid: need explorer pool or credentials".into())
                        })?
                };
                let (res, total) = pg_v2_grid(
                    &*pool,
                    &body.schema,
                    &body.table,
                    limit,
                    offset,
                    body.sort_column.as_deref(),
                    body.sort_desc,
                    body.filter_column.as_deref(),
                    body.filter_value.as_ref(),
                )
                .await
                .map_err(Self::map_host_db)?;
                Ok(HostDbV2GridView {
                    result: res,
                    total_count: total,
                })
            }
            "mysql" => {
                let url = if let Some(c) = creds {
                    mysql_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?
                } else {
                    mysql_explorer_url_from_env(&self.host_env_path)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?
                };
                let (res, total) = mysql_v2_grid(
                    &url,
                    &body.schema,
                    &body.table,
                    limit,
                    offset,
                    body.sort_column.as_deref(),
                    body.sort_desc,
                    body.filter_column.as_deref(),
                    body.filter_value.as_ref(),
                )
                .await
                .map_err(Self::map_host_db)?;
                Ok(HostDbV2GridView {
                    result: res,
                    total_count: total,
                })
            }
            _ => Err(ControlError::HostDb(
                "v2 data grid not supported for this engine (use v1 row preview / SQL query)".into(),
            )),
        }
    }

    /// Structured single-row / small batch mutation (engine-specific).
    pub async fn host_db_v2_row_mutate(
        &self,
        instance_id: &str,
        body: &HostDbV2RowMutationBody,
        creds: Option<&HostDbRequestCredentials>,
        write_enabled: bool,
    ) -> Result<HostDbV2MutationResultView, ControlError> {
        if !write_enabled {
            return Err(ControlError::HostDb(
                "host DB writes are disabled (set CONTROL_API_HOST_DB_WRITE=1 on control-api)"
                    .into(),
            ));
        }
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        match engine.as_str() {
            "postgresql" => {
                let pool: Arc<PgPool> = if let Some(c) = creds {
                    let url = postgres_url_with_creds(instance_id, &c.user, &c.pass, "postgres")
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?;
                    Arc::new(
                        connect_ephemeral_postgres(&url)
                            .await
                            .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?,
                    )
                } else {
                    return Err(ControlError::HostDb(
                        "PostgreSQL mutations require per-request X-Pirate-Db-* credentials"
                            .into(),
                    ));
                };
                let n = pg_row_mutate(
                    &*pool,
                    &body.op,
                    &body.schema,
                    &body.table,
                    body.pk.as_ref(),
                    &body.row,
                )
                .await
                .map_err(Self::map_host_db)?;
                Ok(HostDbV2MutationResultView {
                    affected: n,
                    message: None,
                })
            }
            "mysql" => {
                let url = if let Some(c) = creds {
                    mysql_url_with_creds(instance_id, &c.user, &c.pass)
                        .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))?
                } else {
                    return Err(ControlError::HostDb(
                        "MySQL mutations require per-request X-Pirate-Db-* credentials"
                            .into(),
                    ));
                };
                let n = mysql_row_mutate(
                    &url,
                    &body.op,
                    &body.schema,
                    &body.table,
                    body.pk.as_ref(),
                    &body.row,
                )
                .await
                .map_err(Self::map_host_db)?;
                Ok(HostDbV2MutationResultView {
                    affected: n,
                    message: None,
                })
            }
            _ => Err(ControlError::HostDb(
                "mutations are only supported for postgresql|mysql in this release".into(),
            )),
        }
    }

    /// Read-only: detect Alembic / Flyway / Prisma / Django metadata in the selected database.
    pub async fn host_db_migration_status(
        &self,
        instance_id: &str,
        database: &str,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<HostDbMigrationStatusView, ControlError> {
        let c = creds.ok_or_else(|| {
            ControlError::HostDb(
                "X-Pirate-Db-User and X-Pirate-Db-Password are required for migration status"
                    .into(),
            )
        })?;
        host_migration_status(instance_id, &c.user, &c.pass, database)
            .await
            .map_err(|e: DbHostError| ControlError::HostDb(e.to_string()))
    }

    /// Create a new database. **PostgreSQL:** per-request `X-Pirate-Db-*` (same as create-user; no `PIRATE_POSTGRES_ADMIN_URL`). **MySQL:** host `PIRATE_MYSQL_ADMIN_URL`.
    pub async fn host_db_admin_create_database(
        &self,
        instance_id: &str,
        body: &HostDbCreateDatabaseBody,
        creds: Option<&HostDbRequestCredentials>,
    ) -> Result<(), ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        match engine.as_str() {
            "postgresql" => {
                let c = creds.ok_or_else(|| {
                    ControlError::HostDb(
                        "database credentials are required: set X-Pirate-Db-User and X-Pirate-Db-Password (same as host database browse)"
                            .into(),
                    )
                })?;
                postgres_create_database_for_caller(instance_id, c, body)
                    .await
                    .map_err(|e| ControlError::HostDb(e.to_string()))
            }
            "mysql" => mysql_create_database(&self.host_env_path, body)
                .await
                .map_err(|e| ControlError::HostDb(e.to_string())),
            e => Err(ControlError::HostDb(format!(
                "admin create-database is not supported for {e} instances"
            ))),
        }
    }

    /// `CREATE TABLE` via superuser (allowlisted column types; identifiers validated per engine).
    pub async fn host_db_admin_create_table(
        &self,
        instance_id: &str,
        body: &HostDbAdminCreateTableBody,
    ) -> Result<(), ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        match engine.as_str() {
            "postgresql" => postgres_create_table(&self.host_env_path, body)
                .await
                .map_err(|e| ControlError::HostDb(e.to_string())),
            "mysql" => mysql_create_table(&self.host_env_path, body)
                .await
                .map_err(|e| ControlError::HostDb(e.to_string())),
            e => Err(ControlError::HostDb(format!(
                "admin create-table is not supported for {e} instances"
            ))),
        }
    }

    /// Login role + `GRANT` for application access (password returned once if generated).
    /// Requires per-request DB credentials (same as host-db browse); does not use host env superuser URL.
    pub async fn host_db_admin_create_user(
        &self,
        instance_id: &str,
        body: &HostDbAdminCreateUserBody,
        creds: &HostDbRequestCredentials,
    ) -> Result<HostDbAdminCreateUserView, ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        if engine != "postgresql" {
            return Err(ControlError::HostDb(
                "admin create-user is only for postgresql instances".into(),
            ));
        }
        postgres_create_connection_user(instance_id, creds, body)
            .await
            .map_err(|e| ControlError::HostDb(e.to_string()))
    }

    /// `DROP ROLE` for a login role; optional `DROP OWNED` across all non-template DBs.
    pub async fn host_db_admin_delete_user(
        &self,
        instance_id: &str,
        body: &HostDbAdminDeleteUserBody,
        creds: &HostDbRequestCredentials,
    ) -> Result<HostDbAdminDeleteUserView, ControlError> {
        let (engine, _, _) = parse_instance_id(instance_id)
            .ok_or_else(|| ControlError::HostDb("invalid instance id".into()))?;
        if engine != "postgresql" {
            return Err(ControlError::HostDb(
                "admin delete-user is only for postgresql instances".into(),
            ));
        }
        postgres_delete_connection_user(instance_id, creds, body)
            .await
            .map_err(|e| ControlError::HostDb(e.to_string()))
    }

    /// Whitelisted migration CLI (`alembic` / `npx prisma` / `flyway`) in an allowlisted cwd.
    pub async fn host_db_migration_run(
        &self,
        _instance_id: &str,
        body: &HostDbMigrationRunBody,
    ) -> Result<HostDbMigrationRunView, ControlError> {
        migration_run_approved(&self.host_env_path, body)
            .await
            .map_err(|e| ControlError::HostDb(e.to_string()))
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
        wire_mode: Option<i32>,
        wire_config_json: Option<String>,
        ingress_protocol: Option<i32>,
        ingress_listen_port: Option<u32>,
        ingress_listen_udp_port: Option<u32>,
        ingress_config_json: Option<String>,
        ingress_tls_json: Option<String>,
        ingress_template_version: Option<u32>,
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
            wire_mode,
            wire_config_json,
            ingress_protocol,
            ingress_listen_port,
            ingress_listen_udp_port,
            ingress_config_json,
            ingress_tls_json,
            ingress_template_version,
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

    /// Update managed proxy session policy (and optional wire) via deploy-server `UpdateSettings`.
    pub async fn update_proxy_invitation(
        &self,
        project_id: String,
        session_id: String,
        policy: ProxyConnectionPolicy,
        wire_mode: Option<i32>,
        wire_config_json: Option<String>,
        ingress_protocol: Option<i32>,
        ingress_listen_port: Option<u32>,
        ingress_listen_udp_port: Option<u32>,
        ingress_config_json: Option<String>,
        ingress_tls_json: Option<String>,
        ingress_template_version: Option<u32>,
    ) -> Result<UpdateProxySettingsResponse, ControlError> {
        validate_project_id(&project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(&project_id);
        let sk = self.grpc_signing_key.as_ref().ok_or_else(|| {
            ControlError::Grpc(
                "grpc signing key is not configured; cannot update proxy tunnel invitations"
                    .to_string(),
            )
        })?;
        let mut req = tonic::Request::new(UpdateProxySettingsRequest {
            project_id: pid.clone(),
            session_id,
            policy: Some(policy),
            wire_mode,
            wire_config_json,
            ingress_protocol,
            ingress_listen_port,
            ingress_listen_udp_port,
            ingress_config_json,
            ingress_tls_json,
            ingress_template_version,
        });
        attach_auth_metadata(&mut req, sk, "UpdateSettings", &pid, "")
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        let mut client = DeployServiceClient::connect(self.grpc_endpoint.clone())
            .await
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
        client
            .update_settings(req)
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

    pub async fn fetch_proxy_invitation_by_subscription_token(
        &self,
        token: &str,
    ) -> Result<Option<deploy_db::GrpcProxySessionRow>, ControlError> {
        let db = self.db.as_ref().ok_or_else(|| {
            ControlError::Grpc(
                "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)"
                    .to_string(),
            )
        })?;
        db.fetch_grpc_proxy_session_by_subscription_token(token)
            .await
            .map_err(Into::into)
    }

    /// HTTP ingress: stream a tarball from `artifact_path` to deploy-server (same RPC as desktop gRPC client).
    pub async fn grpc_upload_project_artifact_from_path(
        &self,
        project_id: &str,
        version: &str,
        manifest_toml: Option<&str>,
        artifact_path: &Path,
        file_len: u64,
        chunk_size: usize,
        max_upload_bytes: u64,
    ) -> Result<deploy_proto::deploy::DeployResponse, ControlError> {
        validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
        let pid = normalize_project_id(project_id);
        crate::grpc_artifact_upload::grpc_upload_project_artifact_from_path(
            &self.grpc_endpoint,
            self.grpc_signing_key.as_ref(),
            &pid,
            version,
            manifest_toml,
            artifact_path,
            file_len,
            chunk_size,
            max_upload_bytes,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_releases_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = ControlPlane::new(
            tmp.path().to_path_buf(),
            "http://[::1]:9".into(),
            None,
            None,
            None,
            std::path::PathBuf::from("/dev/null"),
            None,
        );
        let r = p.list_releases("default").unwrap();
        assert!(r.releases.is_empty());
    }

    #[test]
    fn list_releases_sorts_names() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = tmp.path().join("releases");
        fs::create_dir_all(rel.join("v2")).unwrap();
        fs::create_dir_all(rel.join("v1")).unwrap();
        let p = ControlPlane::new(
            tmp.path().to_path_buf(),
            "http://[::1]:9".into(),
            None,
            None,
            None,
            std::path::PathBuf::from("/dev/null"),
            None,
        );
        let r = p.list_releases("default").unwrap();
        assert_eq!(r.releases, vec!["v1", "v2"]);
    }
}
