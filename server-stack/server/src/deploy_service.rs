//! Deploy: stream → temp file, SHA-256, tar unpack, symlink, process control.

use deploy_core::{
    normalize_project_id, project_deploy_root, read_current_version_from_symlink,
    refresh_process_state, release_dir_for_version, validate_project_id,
    validate_version as validate_version_core, AppState,
};
use deploy_db::DbStore;
use crate::auth::ServerAuth;
use deploy_auth::{
    now_unix_ms, signing_payload, verify_rpc_metadata, verify_upload_metadata,
    verify_upload_server_stack_metadata, META_PROJECT, META_VERSION,
};
use deploy_control::{
    collect_cpu_detail, collect_disk_detail, collect_host_stats, collect_memory_detail,
    collect_network_detail, collect_processes_list,
    CpuDetail, CpuTimes, DiskDetail, DiskIoSummary, HostLogLine, HostMountStats, HostNetInterface,
    HostStatsView, LoadAvg, MemoryDetail, MemoryOverview, NetworkDetail, ProcessCpu, ProcessDisk,
    ProcessMem, ProcessRow, ProcessesDetail, SeriesHint, NetCounters,
};
use std::collections::HashMap;
use deploy_proto::deploy::{
    deploy_service_server::DeployService, host_stats_detail_response::Detail as HostStatsDetailOneof,
    proxy_client_msg, proxy_server_msg,
    CpuDetailProto, CpuTimesProto, DeployChunk, DeployResponse, DiskDetailProto, DiskIoSummaryProto,
    HostLogLineProto, HostMountStatsProto, HostNetInterfaceProto, HostStatsDetailKind,
    HostStatsDetailRequest, HostStatsDetailResponse, HostStatsRequest, HostStatsResponse, LoadAvgProto,
    MemoryDetailProto, MemoryOverviewProto, NetworkDetailProto, PairRequest, PairResponse,
    ProcessCpuProto, ProcessDiskProto, ProcessMemProto, ProcessRowProto, ProcessesDetailProto,
    ProxyClientMsg, ProxyOpenResult, ProxyServerMsg,
    RestartProcessRequest, RollbackRequest, RollbackResponse, SeriesHintProto, ServerStackChunk,
    ServerStackInfo, ServerStackInfoRequest, ServerStackResponse, StatusRequest, StatusResponse,
    StopProcessRequest,
};
use futures_util::Stream;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::auth::{sign_pair_response, verify_pair_signature};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{error, info, warn};

fn validate_version(version: &str) -> Result<(), Status> {
    validate_version_core(version).map_err(|e| Status::invalid_argument(e))
}

#[derive(Clone)]
pub struct DeployServiceImpl {
    /// Base path; each project uses [`project_deploy_root`] under this path.
    pub base_root: PathBuf,
    pub max_upload_bytes: u64,
    pub max_server_stack_bytes: u64,
    pub allow_server_stack_update: bool,
    pub binary_fallback: String,
    /// gRPC endpoint URL for local client bundles (`DEPLOY_GRPC_PUBLIC_URL` / default).
    pub public_url: String,
    pub states: Arc<tokio::sync::Mutex<HashMap<String, AppState>>>,
    pub db: Option<Arc<DbStore>>,
    pub auth: Option<Arc<ServerAuth>>,
    /// Previous network counters for host stats / network detail (same process as control-api).
    pub host_net: Arc<std::sync::Mutex<Option<NetCounters>>>,
    /// Optional app log path for `log_tail` in host stats (`DEPLOY_HOST_STATS_LOG_TAIL`).
    pub log_tail_path: Option<PathBuf>,
}

impl DeployServiceImpl {
    pub fn new(
        base_root: PathBuf,
        max_upload_bytes: u64,
        max_server_stack_bytes: u64,
        allow_server_stack_update: bool,
        binary_fallback: String,
        public_url: String,
        states: Arc<tokio::sync::Mutex<HashMap<String, AppState>>>,
        db: Option<Arc<DbStore>>,
        auth: Option<Arc<ServerAuth>>,
        host_net: Arc<std::sync::Mutex<Option<NetCounters>>>,
        log_tail_path: Option<PathBuf>,
    ) -> Self {
        Self {
            base_root,
            max_upload_bytes,
            max_server_stack_bytes,
            allow_server_stack_update,
            binary_fallback,
            public_url,
            states,
            db,
            auth,
            host_net,
            log_tail_path,
        }
    }

    fn status_response(&self, current_version: String, state: String) -> StatusResponse {
        let (client_connect_token, client_connect_pairing) = if let Some(ref auth) = self.auth {
            (
                auth.server_pubkey_b64.clone(),
                auth.pairing_code.read().clone(),
            )
        } else {
            (String::new(), String::new())
        };
        StatusResponse {
            current_version,
            state,
            client_connect_token,
            client_connect_url: self.public_url.clone(),
            client_connect_pairing,
        }
    }

    fn staging_dir(project_root: &Path) -> PathBuf {
        project_root.join(".staging")
    }

    fn spawn_db_record(
        &self,
        project_id: &str,
        kind: &'static str,
        deployed_version: &str,
        current_version: &str,
        state: &str,
        last_error: Option<&str>,
    ) {
        let Some(db) = self.db.clone() else {
            return;
        };
        let pid = project_id.to_string();
        let deployed_version = deployed_version.to_string();
        let current_version = current_version.to_string();
        let state = state.to_string();
        let last_err = last_error.map(|s| s.to_string());
        let snapshot = format!("{state}|{current_version}|{:?}", last_err);
        tokio::spawn(async move {
            if let Err(e) = db
                .record_event(&pid, kind, &deployed_version, Some(&snapshot))
                .await
            {
                error!(%e, "deploy_db record_event");
            }
            if let Err(e) = db
                .upsert_snapshot(&pid, &current_version, &state, last_err.as_deref())
                .await
            {
                error!(%e, "deploy_db upsert_snapshot");
            }
        });
    }
}

#[cfg(unix)]
fn set_current_symlink(root: &Path, version: &str) -> std::io::Result<()> {
    use std::os::unix::fs::symlink;
    let current = root.join("current");
    let rel = Path::new("releases").join(version);
    let tmp = root.join(".current.tmp");
    let _ = std::fs::remove_file(&tmp);
    symlink(&rel, &tmp)?;
    std::fs::rename(&tmp, &current)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_current_symlink(_root: &Path, _version: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink deploy only supported on Unix",
    ))
}

#[cfg(unix)]
fn ensure_run_sh_executable(release_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let run = release_dir.join("run.sh");
    if run.exists() {
        if let Ok(meta) = std::fs::metadata(&run) {
            let mut p = meta.permissions();
            p.set_mode(0o755);
            let _ = std::fs::set_permissions(&run, p);
        }
    }
}

#[cfg(not(unix))]
fn ensure_run_sh_executable(_release_dir: &Path) {}

fn unpack_tar_gz(src: &Path, dst: &Path) -> std::io::Result<()> {
    use flate2::read::GzDecoder;
    use std::fs::File;
    use std::path::Component;
    use tar::Archive;

    let file = File::open(src)?;
    let dec = GzDecoder::new(file);
    let mut archive = Archive::new(dec);
    std::fs::create_dir_all(dst)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "path traversal in tar entry",
            ));
        }
        entry.unpack_in(dst)?;
    }
    Ok(())
}

async fn spawn_release(
    root: &Path,
    version: &str,
    binary_fallback: &str,
) -> Result<tokio::process::Child, Status> {
    let release_dir = release_dir_for_version(root, version);
    let run_sh = release_dir.join("run.sh");

    let mut cmd = if run_sh.exists() {
        let mut c = tokio::process::Command::new("sh");
        c.arg(run_sh.as_os_str())
            .current_dir(&release_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        c
    } else {
        let bin = release_dir.join(binary_fallback);
        if !bin.exists() {
            return Err(Status::failed_precondition(format!(
                "neither run.sh nor {}/{} found",
                release_dir.display(),
                binary_fallback
            )));
        }
        let mut c = tokio::process::Command::new(&bin);
        c.current_dir(&release_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        c
    };

    cmd.spawn()
        .map_err(|e| Status::internal(format!("spawn failed: {e}")))
}

async fn stop_child(child: &mut tokio::process::Child) {
    if let Err(e) = child.kill().await {
        warn!(error = %e, "kill child");
    }
    let _ = child.wait().await;
}

#[tonic::async_trait]
impl DeployService for DeployServiceImpl {
    type ProxyTunnelStream =
        Pin<Box<dyn Stream<Item = Result<ProxyServerMsg, Status>> + Send + 'static>>;

    async fn pair(
        &self,
        request: Request<PairRequest>,
    ) -> Result<Response<PairResponse>, Status> {
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; pairing unavailable")
        })?;
        let _ = auth
            .reload_pairing_code()
            .map_err(|e| Status::internal(e.to_string()))?;
        let r = request.into_inner();
        if r.client_public_key_b64.is_empty()
            || r.nonce.is_empty()
            || r.client_signature_b64.is_empty()
        {
            return Err(Status::invalid_argument("missing pair fields"));
        }
        let now = now_unix_ms();
        if (now - r.timestamp_ms).abs() > auth.config.max_clock_skew_ms {
            return Err(Status::deadline_exceeded("timestamp skew"));
        }
        auth.verify_pairing(&r.pairing_code)?;
        verify_pair_signature(
            &r.client_public_key_b64,
            &auth.server_pubkey_b64,
            r.timestamp_ms,
            &r.nonce,
            &r.pairing_code,
            &r.client_signature_b64,
        )?;
        auth.add_peer(&r.client_public_key_b64)?;
        let server_sig = sign_pair_response(
            &auth.signing_key,
            &auth.server_pubkey_b64,
            &r.client_public_key_b64,
            r.timestamp_ms,
            &r.nonce,
        );
        Ok(Response::new(PairResponse {
            server_public_key_b64: auth.server_pubkey_b64.clone(),
            server_signature_b64: server_sig,
            status: "paired".to_string(),
        }))
    }

    async fn upload(
        &self,
        request: Request<Streaming<DeployChunk>>,
    ) -> Result<Response<DeployResponse>, Status> {
        let meta = request.metadata().clone();
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_upload_metadata(&meta, &peers, &auth.config, &auth.nonce_tracker)
                .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }
        let expected_version = meta
            .get(META_VERSION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let mut stream = request.into_inner();

        let mut version: Option<String> = None;
        let mut project_key: Option<String> = None;
        let mut project_root: Option<PathBuf> = None;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut expected_sha_hex: Option<String> = None;
        let mut temp_path: Option<PathBuf> = None;
        let mut file: Option<tokio::fs::File> = None;

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| Status::internal(e.to_string()))?;

            if version.is_none() {
                if chunk.version.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument("first chunk must set version"));
                }
                if let Some(ref ev) = expected_version {
                    if chunk.version != *ev {
                        if let Some(ref p) = temp_path {
                            let _ = tokio::fs::remove_file(p).await;
                        }
                        return Err(Status::invalid_argument(
                            "first chunk version must match x-deploy-version metadata",
                        ));
                    }
                }
                validate_version(&chunk.version)?;
                validate_project_id(&chunk.project_id).map_err(Status::invalid_argument)?;
                let chunk_proj = normalize_project_id(&chunk.project_id);
                let meta_proj = meta.get(META_PROJECT).and_then(|v| v.to_str().ok());
                match meta_proj {
                    Some(m) => {
                        if normalize_project_id(m) != chunk_proj {
                            return Err(Status::invalid_argument(
                                "project_id mismatch between metadata and first chunk",
                            ));
                        }
                    }
                    None => {
                        if chunk_proj != "default" {
                            return Err(Status::invalid_argument(
                                "non-default project_id requires x-deploy-project metadata",
                            ));
                        }
                    }
                }

                let root = project_deploy_root(&self.base_root, &chunk.project_id);
                tokio::fs::create_dir_all(Self::staging_dir(&root))
                    .await
                    .map_err(|e| Status::internal(format!("staging dir: {e}")))?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let tp = Self::staging_dir(&root).join(format!("upload_{stamp}.tar.gz"));
                let f = tokio::fs::File::create(&tp)
                    .await
                    .map_err(|e| Status::internal(format!("temp file: {e}")))?;
                file = Some(f);
                temp_path = Some(tp);
                project_root = Some(root);
                project_key = Some(chunk_proj);
                version = Some(chunk.version.clone());
            } else if !chunk.version.is_empty()
                && chunk.version.as_str() != version.as_deref().unwrap()
            {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::invalid_argument("version mismatch between chunks"));
            }

            let n = chunk.data.len() as u64;
            if written.saturating_add(n) > self.max_upload_bytes {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::resource_exhausted(format!(
                    "artifact exceeds limit of {} bytes",
                    self.max_upload_bytes
                )));
            }

            hasher.update(&chunk.data);
            file.as_mut()
                .ok_or_else(|| Status::internal("internal: file not open"))?
                .write_all(&chunk.data)
                .await
                .map_err(|e| Status::internal(format!("write: {e}")))?;
            written = written.saturating_add(n);

            if chunk.is_last {
                if chunk.sha256_hex.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument(
                        "sha256_hex required when is_last is true",
                    ));
                }
                expected_sha_hex = Some(chunk.sha256_hex.clone());
            }
        }

        let version = version.ok_or_else(|| {
            if let Some(ref p) = temp_path {
                let _ = std::fs::remove_file(p);
            }
            Status::invalid_argument("no version in stream")
        })?;
        let project_key = project_key.ok_or_else(|| Status::invalid_argument("no project in stream"))?;
        let project_root = project_root.ok_or_else(|| Status::internal("internal: no project root"))?;
        let temp_path = temp_path.ok_or_else(|| Status::internal("internal: no temp path"))?;

        let expected_hex = expected_sha_hex.ok_or_else(|| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("stream must end with is_last=true and sha256_hex")
        })?;

        let digest = hasher.finalize();
        let expected_bytes = hex::decode(expected_hex.trim()).map_err(|_| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("invalid sha256_hex (not hex)")
        })?;
        if expected_bytes.len() != 32 {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("sha256 must be 32 bytes"));
        }
        if digest.as_slice() != expected_bytes.as_slice() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("SHA-256 mismatch"));
        }

        let release_dir = release_dir_for_version(&project_root, &version);
        tokio::fs::create_dir_all(deploy_core::releases_dir(&project_root))
            .await
            .map_err(|e| Status::internal(format!("releases dir: {e}")))?;

        let tp = temp_path.clone();
        let rd = release_dir.clone();
        tokio::task::spawn_blocking(move || {
            if rd.exists() {
                std::fs::remove_dir_all(&rd).map_err(|e| e.to_string())?;
            }
            std::fs::create_dir_all(&rd).map_err(|e| e.to_string())?;
            unpack_tar_gz(&tp, &rd).map_err(|e| e.to_string())?;
            ensure_run_sh_executable(&rd);
            std::fs::remove_file(&tp).map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?
        .map_err(|e| Status::internal(e))?;

        info!(project = %project_key, version = %version, "artifact unpacked");

        let root = project_root.clone();
        let bf = self.binary_fallback.clone();
        let mut map = self.states.lock().await;
        let st = map.entry(project_key.clone()).or_insert_with(AppState::default);

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }

        if let Err(e) = set_current_symlink(&root, &version) {
            error!(error = %e, "symlink");
            st.state = "error".to_string();
            st.last_error = Some(e.to_string());
            return Err(Status::internal(format!("symlink: {e}")));
        }

        match spawn_release(&root, &version, &bf).await {
            Ok(child) => {
                st.child = Some(child);
                st.current_version = version.clone();
                st.state = "running".to_string();
                st.last_error = None;
                info!(project = %project_key, version = %version, "deployed and started");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(map);
                self.spawn_db_record(
                    &project_key,
                    "upload",
                    &version,
                    &cur,
                    &state,
                    err.as_deref(),
                );
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.message().to_string());
                return Err(e);
            }
        }

        Ok(Response::new(DeployResponse {
            status: "ok".to_string(),
            deployed_version: version,
        }))
    }

    async fn get_status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let meta = request.metadata().clone();
        let inner = request.into_inner();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let key = normalize_project_id(&inner.project_id);
        let sign_payload = signing_payload("GetStatus", &inner.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetStatus",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }
        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);
        refresh_process_state(st);

        let mut current = st.current_version.clone();
        if current.is_empty() {
            if let Some(v) = read_current_version_from_symlink(&root) {
                current = v;
            }
        }

        Ok(Response::new(
            self.status_response(current, st.state.clone()),
        ))
    }

    async fn rollback(
        &self,
        request: Request<RollbackRequest>,
    ) -> Result<Response<RollbackResponse>, Status> {
        let meta = request.metadata().clone();
        let inner = request.into_inner();
        let v = inner.version;
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let key = normalize_project_id(&inner.project_id);
        let sign_payload = signing_payload("Rollback", &inner.project_id, &v);
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "Rollback",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }
        validate_version(&v)?;

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let target = release_dir_for_version(&root, &v);
        if !target.is_dir() {
            return Err(Status::not_found(format!("release {v} not found")));
        }

        let bf = self.binary_fallback.clone();
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }

        if let Err(e) = set_current_symlink(&root, &v) {
            error!(error = %e, "rollback symlink");
            st.state = "error".to_string();
            st.last_error = Some(e.to_string());
            return Err(Status::internal(format!("symlink: {e}")));
        }

        match spawn_release(&root, &v, &bf).await {
            Ok(child) => {
                st.child = Some(child);
                st.current_version = v.clone();
                st.state = "running".to_string();
                st.last_error = None;
                info!(project = %key, version = %v, "rollback complete");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(map);
                self.spawn_db_record(&key, "rollback", &v, &cur, &state, err.as_deref());
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.message().to_string());
                return Err(e);
            }
        }

        Ok(Response::new(RollbackResponse {
            status: "ok".to_string(),
            active_version: v,
        }))
    }

    async fn stop_process(
        &self,
        request: Request<StopProcessRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let meta = request.metadata().clone();
        let inner = request.into_inner();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let key = normalize_project_id(&inner.project_id);
        let sign_payload = signing_payload("StopProcess", &inner.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "StopProcess",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);
        refresh_process_state(st);

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }
        st.state = "stopped".to_string();
        st.last_error = None;

        let mut current = st.current_version.clone();
        if current.is_empty() {
            if let Some(v) = read_current_version_from_symlink(&root) {
                current = v;
                st.current_version = current.clone();
            }
        }

        let cur = st.current_version.clone();
        let state = st.state.clone();
        drop(map);
        self.spawn_db_record(&key, "stop", &cur, &cur, &state, None);

        Ok(Response::new(
            self.status_response(current, "stopped".to_string()),
        ))
    }

    async fn restart_process(
        &self,
        request: Request<RestartProcessRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let meta = request.metadata().clone();
        let inner = request.into_inner();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let key = normalize_project_id(&inner.project_id);
        let sign_payload = signing_payload("RestartProcess", &inner.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "RestartProcess",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let bf = self.binary_fallback.clone();
        let mut map = self.states.lock().await;
        let st = map.entry(key.clone()).or_insert_with(AppState::default);
        refresh_process_state(st);

        let mut ver = st.current_version.clone();
        if ver.is_empty() {
            ver = read_current_version_from_symlink(&root).unwrap_or_default();
        }
        if ver.is_empty() {
            return Err(Status::failed_precondition(
                "no active release; deploy or rollback first",
            ));
        }

        let target = release_dir_for_version(&root, &ver);
        if !target.is_dir() {
            return Err(Status::failed_precondition(format!(
                "release directory for {ver} missing"
            )));
        }

        if let Some(ref mut c) = st.child {
            stop_child(c).await;
            st.child = None;
        }

        match spawn_release(&root, &ver, &bf).await {
            Ok(child) => {
                st.child = Some(child);
                st.current_version = ver.clone();
                st.state = "running".to_string();
                st.last_error = None;
                info!(project = %key, version = %ver, "process restarted");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(map);
                self.spawn_db_record(&key, "restart", &ver, &cur, &state, err.as_deref());
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.message().to_string());
                return Err(e);
            }
        }

        let map = self.states.lock().await;
        let st = map.get(&key).ok_or_else(|| Status::internal("internal: project state"))?;
        Ok(Response::new(self.status_response(
            st.current_version.clone(),
            st.state.clone(),
        )))
    }

    async fn get_host_stats(
        &self,
        request: Request<HostStatsRequest>,
    ) -> Result<Response<HostStatsResponse>, Status> {
        let meta = request.metadata().clone();
        let inner = request.into_inner();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("GetHostStats", &inner.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetHostStats",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }

        let root = project_deploy_root(&self.base_root, &inner.project_id);
        let host_net = self.host_net.clone();
        let log_tail = self.log_tail_path.clone();

        let view = tokio::task::spawn_blocking(move || {
            let prev = host_net.lock().unwrap().clone();
            let (stats, net) =
                collect_host_stats(&root, prev.as_ref(), log_tail.as_deref());
            *host_net.lock().unwrap() = Some(net);
            stats
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(host_stats_view_to_proto(&view)))
    }

    async fn get_host_stats_detail(
        &self,
        request: Request<HostStatsDetailRequest>,
    ) -> Result<Response<HostStatsDetailResponse>, Status> {
        let meta = request.metadata().clone();
        let inner = request.into_inner();
        validate_project_id(&inner.project_id).map_err(Status::invalid_argument)?;
        let sign_payload = signing_payload("GetHostStatsDetail", &inner.project_id, "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetHostStatsDetail",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }

        if inner.kind == HostStatsDetailKind::Unspecified as i32 {
            return Err(Status::invalid_argument("kind is required"));
        }

        let top = inner.top.clamp(5, 100) as usize;
        let limit = inner.limit.clamp(10, 2000) as usize;
        let q = inner.q.clone();
        let kind = inner.kind;

        let host_net = self.host_net.clone();

        let detail = tokio::task::spawn_blocking(move || match kind {
            k if k == HostStatsDetailKind::HostStatsDetailCpu as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Cpu(cpu_detail_to_proto(
                    &collect_cpu_detail(top),
                ))),
            },
            k if k == HostStatsDetailKind::HostStatsDetailMemory as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Memory(memory_detail_to_proto(
                    &collect_memory_detail(top),
                ))),
            },
            k if k == HostStatsDetailKind::HostStatsDetailDisk as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Disk(disk_detail_to_proto(
                    &collect_disk_detail(top),
                ))),
            },
            k if k == HostStatsDetailKind::HostStatsDetailNetwork as i32 => {
                let prev = host_net.lock().unwrap().clone();
                let (d, net) = collect_network_detail(prev.as_ref());
                *host_net.lock().unwrap() = Some(net);
                HostStatsDetailResponse {
                    detail: Some(HostStatsDetailOneof::Network(network_detail_to_proto(&d))),
                }
            }
            k if k == HostStatsDetailKind::HostStatsDetailProcesses as i32 => HostStatsDetailResponse {
                detail: Some(HostStatsDetailOneof::Processes(processes_detail_to_proto(
                    &collect_processes_list(&q, limit),
                ))),
            },
            _ => HostStatsDetailResponse { detail: None },
        })
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

        if detail.detail.is_none() {
            return Err(Status::invalid_argument("invalid kind"));
        }

        Ok(Response::new(detail))
    }

    async fn upload_server_stack(
        &self,
        request: Request<Streaming<ServerStackChunk>>,
    ) -> Result<Response<ServerStackResponse>, Status> {
        if !self.allow_server_stack_update {
            return Err(Status::failed_precondition(
                "server stack OTA disabled; set DEPLOY_ALLOW_SERVER_STACK_UPDATE=1 on deploy-server",
            ));
        }
        let meta = request.metadata().clone();
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_upload_server_stack_metadata(&meta, &peers, &auth.config, &auth.nonce_tracker)
                .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }
        let expected_version = meta
            .get(META_VERSION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let mut stream = request.into_inner();
        let mut version: Option<String> = None;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut expected_sha_hex: Option<String> = None;
        let mut temp_path: Option<PathBuf> = None;
        let mut file: Option<tokio::fs::File> = None;

        let staging_base = self.base_root.join(".stack-staging");
        tokio::fs::create_dir_all(&staging_base)
            .await
            .map_err(|e| Status::internal(format!("stack staging: {e}")))?;

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| Status::internal(e.to_string()))?;

            if version.is_none() {
                if chunk.version.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument("first chunk must set version"));
                }
                if let Some(ref ev) = expected_version {
                    if chunk.version != *ev {
                        if let Some(ref p) = temp_path {
                            let _ = tokio::fs::remove_file(p).await;
                        }
                        return Err(Status::invalid_argument(
                            "first chunk version must match x-deploy-version metadata",
                        ));
                    }
                }
                validate_version(&chunk.version)?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let tp = staging_base.join(format!("stack_upload_{stamp}.tar.gz"));
                let f = tokio::fs::File::create(&tp)
                    .await
                    .map_err(|e| Status::internal(format!("temp file: {e}")))?;
                file = Some(f);
                temp_path = Some(tp);
                version = Some(chunk.version.clone());
            } else if !chunk.version.is_empty()
                && chunk.version.as_str() != version.as_deref().unwrap()
            {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::invalid_argument("version mismatch between chunks"));
            }

            let n = chunk.data.len() as u64;
            if written.saturating_add(n) > self.max_server_stack_bytes {
                if let Some(ref p) = temp_path {
                    let _ = tokio::fs::remove_file(p).await;
                }
                return Err(Status::resource_exhausted(format!(
                    "server stack artifact exceeds limit of {} bytes",
                    self.max_server_stack_bytes
                )));
            }

            hasher.update(&chunk.data);
            file.as_mut()
                .ok_or_else(|| Status::internal("internal: file not open"))?
                .write_all(&chunk.data)
                .await
                .map_err(|e| Status::internal(format!("write: {e}")))?;
            written = written.saturating_add(n);

            if chunk.is_last {
                if chunk.sha256_hex.is_empty() {
                    if let Some(ref p) = temp_path {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                    return Err(Status::invalid_argument(
                        "sha256_hex required when is_last is true",
                    ));
                }
                expected_sha_hex = Some(chunk.sha256_hex.clone());
            }
        }

        let version = version.ok_or_else(|| {
            if let Some(ref p) = temp_path {
                let _ = std::fs::remove_file(p);
            }
            Status::invalid_argument("no version in stream")
        })?;
        let temp_path = temp_path.ok_or_else(|| Status::internal("internal: no temp path"))?;
        let expected_hex = expected_sha_hex.ok_or_else(|| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("stream must end with is_last=true and sha256_hex")
        })?;

        let digest = hasher.finalize();
        let expected_bytes = hex::decode(expected_hex.trim()).map_err(|_| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("invalid sha256_hex (not hex)")
        })?;
        if expected_bytes.len() != 32 {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("sha256 must be 32 bytes"));
        }
        if digest.as_slice() != expected_bytes.as_slice() {
            let _ = std::fs::remove_file(&temp_path);
            return Err(Status::invalid_argument("SHA-256 mismatch"));
        }

        let extract_dir = staging_base.join(format!("extract_{}", version.replace(['/', '\\'], "_")));
        if extract_dir.exists() {
            let _ = tokio::fs::remove_dir_all(&extract_dir).await;
        }
        tokio::fs::create_dir_all(&extract_dir)
            .await
            .map_err(|e| Status::internal(format!("extract dir: {e}")))?;

        let tp = temp_path.clone();
        let ed = extract_dir.clone();
        tokio::task::spawn_blocking(move || unpack_tar_gz(&tp, &ed))
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .map_err(|e| Status::internal(e.to_string()))?;

        let bundle_root = match find_pirate_bundle_root(&extract_dir) {
            Ok(p) => p,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                let _ = tokio::fs::remove_dir_all(&extract_dir).await;
                return Err(e);
            }
        };

        let manifest_path = bundle_root.join("server-stack-manifest.json");
        let (ds_ver, ca_ver) = if manifest_path.exists() {
            match std::fs::read_to_string(&manifest_path) {
                Ok(s) => parse_stack_manifest_versions(&s),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

        let br = bundle_root.clone();
        let ver = version.clone();
        let status = tokio::process::Command::new("sudo")
            .arg("/usr/local/lib/pirate/pirate-apply-stack-bundle.sh")
            .arg(&br)
            .arg(&ver)
            .status()
            .await
            .map_err(|e| format!("sudo apply stack: {e}"));

        let _ = tokio::fs::remove_file(&temp_path).await;
        let _ = tokio::fs::remove_dir_all(&extract_dir).await;

        match status {
            Ok(s) if s.success() => {
                info!(version = %ver, "server stack OTA applied");
                Ok(Response::new(ServerStackResponse {
                    status: "ok".to_string(),
                    applied_version: ver.clone(),
                    deploy_server_pkg_version: ds_ver,
                    control_api_pkg_version: ca_ver,
                }))
            }
            Ok(s) => Err(Status::internal(format!(
                "pirate-apply-stack-bundle.sh exited with {}",
                s.code().unwrap_or(-1)
            ))),
            Err(e) => Err(Status::internal(e)),
        }
    }

    async fn get_server_stack_info(
        &self,
        request: Request<ServerStackInfoRequest>,
    ) -> Result<Response<ServerStackInfo>, Status> {
        let meta = request.metadata().clone();
        let sign_payload = signing_payload("GetServerStackInfo", "", "");
        if let Some(ref auth) = self.auth {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "GetServerStackInfo",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }

        let root = PathBuf::from("/var/lib/pirate");
        let ver_path = root.join("server-stack-version");
        let bundle_version = tokio::fs::read_to_string(&ver_path)
            .await
            .unwrap_or_default()
            .trim()
            .to_string();

        let manifest_path = root.join("server-stack-manifest.json");
        let manifest_json = if manifest_path.exists() {
            tokio::fs::read_to_string(&manifest_path)
                .await
                .ok()
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        };

        let deploy_server_binary_version = Some(env!("CARGO_PKG_VERSION").to_string());

        Ok(Response::new(ServerStackInfo {
            bundle_version,
            manifest_json,
            deploy_server_binary_version,
        }))
    }

    async fn proxy_tunnel(
        &self,
        request: Request<Streaming<ProxyClientMsg>>,
    ) -> Result<Response<Self::ProxyTunnelStream>, Status> {
        let auth = self.auth.as_ref().ok_or_else(|| {
            Status::failed_precondition("authentication disabled; ProxyTunnel unavailable")
        })?;
        let meta = request.metadata().clone();
        let sign_payload = signing_payload("ProxyTunnel", "default", "");
        {
            let peers = auth.peers.read();
            verify_rpc_metadata(
                &meta,
                &peers,
                "ProxyTunnel",
                &sign_payload,
                &auth.config,
                &auth.nonce_tracker,
            )
            .map_err(|e| Status::unauthenticated(e.to_string()))?;
        }

        let mut inbound = request.into_inner();
        let first = inbound
            .message()
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::invalid_argument("empty client stream"))?;

        let open = match first.body {
            Some(proxy_client_msg::Body::Open(o)) => o,
            _ => {
                return Err(Status::invalid_argument(
                    "first ProxyClientMsg must be Open { host, port }",
                ));
            }
        };

        let host = open.host.trim();
        if host.is_empty() {
            return Err(Status::invalid_argument("proxy host is empty"));
        }
        if open.port == 0 || open.port > 65535 {
            return Err(Status::invalid_argument("invalid proxy port"));
        }
        proxy_allowlist_check(host)?;

        let addr = format!("{}:{}", host, open.port);
        let (tx, rx) = mpsc::channel::<Result<ProxyServerMsg, Status>>(64);

        tokio::spawn(async move {
            let tcp = match tokio::time::timeout(
                Duration::from_secs(30),
                tokio::net::TcpStream::connect(addr),
            )
            .await
            {
                Err(_) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                                ok: false,
                                error: "connect timeout".to_string(),
                            })),
                        }))
                        .await;
                    return;
                }
                Ok(Err(e)) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                                ok: false,
                                error: e.to_string(),
                            })),
                        }))
                        .await;
                    return;
                }
                Ok(Ok(s)) => s,
            };

            if tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(ProxyOpenResult {
                        ok: true,
                        error: String::new(),
                    })),
                }))
                .await
                .is_err()
            {
                return;
            }

            const MAX_PROXY_CHUNK: usize = 256 * 1024;
            let (mut tcp_read, mut tcp_write) = tcp.into_split();

            let mut inbound = inbound;
            let t_in = tokio::spawn(async move {
                while let Some(item) = inbound.next().await {
                    let msg = match item {
                        Ok(m) => m,
                        Err(e) => {
                            let _ = tcp_write.shutdown().await;
                            return Err(Status::internal(e.to_string()));
                        }
                    };
                    match msg.body {
                        Some(proxy_client_msg::Body::Open(_)) => {
                            let _ = tcp_write.shutdown().await;
                            return Err(Status::invalid_argument("duplicate Open"));
                        }
                        Some(proxy_client_msg::Body::Data(data)) => {
                            if data.len() > MAX_PROXY_CHUNK {
                                let _ = tcp_write.shutdown().await;
                                return Err(Status::invalid_argument("proxy chunk too large"));
                            }
                            if let Err(e) = tcp_write.write_all(&data).await {
                                let _ = tcp_write.shutdown().await;
                                return Err(Status::internal(e.to_string()));
                            }
                        }
                        Some(proxy_client_msg::Body::Fin(_)) => {
                            let _ = tcp_write.shutdown().await;
                            return Ok(());
                        }
                        None => {}
                    }
                }
                let _ = tcp_write.shutdown().await;
                Ok(())
            });

            let mut buf = vec![0u8; MAX_PROXY_CHUNK];
            let tx_out = tx.clone();
            let t_out = tokio::spawn(async move {
                loop {
                    match tcp_read.read(&mut buf).await {
                        Ok(0) => {
                            let _ = tx_out
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::Eof(true)),
                                }))
                                .await;
                            break;
                        }
                        Ok(n) => {
                            let chunk = buf[..n].to_vec();
                            if tx_out
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::Data(chunk)),
                                }))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx_out
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::Error(e.to_string())),
                                }))
                                .await;
                            break;
                        }
                    }
                }
            });

            let (r_in, _r_out) = tokio::join!(t_in, t_out);
            match r_in {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let _ = tx.send(Err(e)).await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("proxy client task: {e}"))))
                        .await;
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))
            as Pin<Box<dyn Stream<Item = Result<ProxyServerMsg, Status>> + Send + 'static>>))
    }
}

fn proxy_allowlist_check(host: &str) -> Result<(), Status> {
    let Ok(list) = std::env::var("DEPLOY_PROXY_ALLOWLIST") else {
        return Ok(());
    };
    let list = list.trim();
    if list.is_empty() || list == "*" {
        return Ok(());
    }
    let host_lc = host.to_ascii_lowercase();
    for part in list.split(',') {
        let p = part.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if host_lc == p || host_lc.ends_with(&format!(".{p}")) {
            return Ok(());
        }
    }
    Err(Status::permission_denied(
        "proxy target host not allowed by DEPLOY_PROXY_ALLOWLIST",
    ))
}

fn find_pirate_bundle_root(extracted: &Path) -> Result<PathBuf, Status> {
    let d1 = extracted.join("pirate-linux-amd64");
    if d1.join("bin/deploy-server").is_file() && d1.join("bin/control-api").is_file() {
        return Ok(d1);
    }
    if extracted.join("bin/deploy-server").is_file() && extracted.join("bin/control-api").is_file() {
        return Ok(extracted.to_path_buf());
    }
    Err(Status::invalid_argument(
        "expected bundle with bin/deploy-server and bin/control-api (e.g. pirate-linux-amd64/)",
    ))
}

fn parse_stack_manifest_versions(s: &str) -> (Option<String>, Option<String>) {
    let Some(v) = serde_json::from_str::<serde_json::Value>(s).ok() else {
        return (None, None);
    };
    (
        v.get("deploy_server")
            .and_then(|x| x.as_str())
            .map(String::from),
        v.get("control_api").and_then(|x| x.as_str()).map(String::from),
    )
}

fn host_mount_to_proto(m: &HostMountStats) -> HostMountStatsProto {
    HostMountStatsProto {
        path: m.path.clone(),
        total_bytes: m.total_bytes,
        free_bytes: m.free_bytes,
    }
}

fn host_net_to_proto(n: &HostNetInterface) -> HostNetInterfaceProto {
    HostNetInterfaceProto {
        name: n.name.clone(),
        rx_bytes_per_s: n.rx_bytes_per_s,
        tx_bytes_per_s: n.tx_bytes_per_s,
        rx_errors: n.rx_errors,
        tx_errors: n.tx_errors,
    }
}

fn host_log_line_to_proto(l: &HostLogLine) -> HostLogLineProto {
    HostLogLineProto {
        ts_ms: l.ts_ms,
        level: l.level.clone(),
        message: l.message.clone(),
    }
}

fn host_stats_view_to_proto(v: &HostStatsView) -> HostStatsResponse {
    HostStatsResponse {
        disk_free_bytes: v.disk_free_bytes,
        disk_total_bytes: v.disk_total_bytes,
        disk_mount_path: v.disk_mount_path.clone(),
        memory_used_bytes: v.memory_used_bytes,
        memory_total_bytes: v.memory_total_bytes,
        cpu_usage_percent: v.cpu_usage_percent,
        load_average_1m: v.load_average_1m,
        load_average_5m: v.load_average_5m,
        load_average_15m: v.load_average_15m,
        temperature_current_celsius: v.temperature_current_celsius,
        temperature_avg_celsius: v.temperature_avg_celsius,
        process_count: v.process_count as u64,
        disk_mounts: v.disk_mounts.iter().map(host_mount_to_proto).collect(),
        network_interfaces: v
            .network_interfaces
            .iter()
            .map(host_net_to_proto)
            .collect(),
        log_tail: v.log_tail.iter().map(host_log_line_to_proto).collect(),
    }
}

fn load_avg_to_proto(l: &LoadAvg) -> LoadAvgProto {
    LoadAvgProto {
        m1: l.m1,
        m5: l.m5,
        m15: l.m15,
    }
}

fn cpu_times_to_proto(t: &CpuTimes) -> CpuTimesProto {
    CpuTimesProto {
        user_ms: t.user_ms,
        system_ms: t.system_ms,
        idle_ms: t.idle_ms,
    }
}

fn process_cpu_to_proto(p: &ProcessCpu) -> ProcessCpuProto {
    ProcessCpuProto {
        pid: p.pid,
        name: p.name.clone(),
        cpu_percent: p.cpu_percent,
    }
}

fn series_hint_to_proto(s: &SeriesHint) -> SeriesHintProto {
    SeriesHintProto {
        available_ranges: s.available_ranges.clone(),
    }
}

fn cpu_detail_to_proto(d: &CpuDetail) -> CpuDetailProto {
    CpuDetailProto {
        ts_ms: d.ts_ms,
        loadavg: Some(load_avg_to_proto(&d.loadavg)),
        times: d.times.as_ref().map(cpu_times_to_proto),
        top_processes: d.top_processes.iter().map(process_cpu_to_proto).collect(),
        series_hint: Some(series_hint_to_proto(&d.series_hint)),
    }
}

fn memory_overview_to_proto(m: &MemoryOverview) -> MemoryOverviewProto {
    MemoryOverviewProto {
        total_bytes: m.total_bytes,
        used_bytes: m.used_bytes,
        available_bytes: m.available_bytes,
        cached_bytes: m.cached_bytes,
        buffers_bytes: m.buffers_bytes,
        swap_total_bytes: m.swap_total_bytes,
        swap_used_bytes: m.swap_used_bytes,
    }
}

fn process_mem_to_proto(p: &ProcessMem) -> ProcessMemProto {
    ProcessMemProto {
        pid: p.pid,
        name: p.name.clone(),
        memory_bytes: p.memory_bytes,
    }
}

fn memory_detail_to_proto(d: &MemoryDetail) -> MemoryDetailProto {
    MemoryDetailProto {
        ts_ms: d.ts_ms,
        memory: Some(memory_overview_to_proto(&d.memory)),
        top_processes: d.top_processes.iter().map(process_mem_to_proto).collect(),
    }
}

fn disk_io_to_proto(d: &DiskIoSummary) -> DiskIoSummaryProto {
    DiskIoSummaryProto {
        note: d.note.to_string(),
    }
}

fn process_disk_to_proto(p: &ProcessDisk) -> ProcessDiskProto {
    ProcessDiskProto {
        pid: p.pid,
        name: p.name.clone(),
        read_bytes: p.read_bytes,
        write_bytes: p.write_bytes,
    }
}

fn disk_detail_to_proto(d: &DiskDetail) -> DiskDetailProto {
    DiskDetailProto {
        ts_ms: d.ts_ms,
        mounts: d.mounts.iter().map(host_mount_to_proto).collect(),
        io: d.io.as_ref().map(disk_io_to_proto),
        top_processes: d.top_processes.iter().map(process_disk_to_proto).collect(),
    }
}

fn network_detail_to_proto(d: &NetworkDetail) -> NetworkDetailProto {
    NetworkDetailProto {
        ts_ms: d.ts_ms,
        interfaces: d.interfaces.iter().map(host_net_to_proto).collect(),
        connections_note: d.connections_note.to_string(),
    }
}

fn process_row_to_proto(p: &ProcessRow) -> ProcessRowProto {
    ProcessRowProto {
        pid: p.pid,
        name: p.name.clone(),
        cpu_percent: p.cpu_percent,
        memory_bytes: p.memory_bytes,
    }
}

fn processes_detail_to_proto(d: &ProcessesDetail) -> ProcessesDetailProto {
    ProcessesDetailProto {
        ts_ms: d.ts_ms,
        processes: d.processes.iter().map(process_row_to_proto).collect(),
        total: d.total as u64,
    }
}
