//! Deploy: stream → temp file, SHA-256, tar unpack, symlink, process control.

use deploy_core::{
    normalize_project_id, project_deploy_root, read_current_version_from_symlink,
    refresh_process_state, release_dir_for_version, validate_project_id,
    validate_version as validate_version_core, AppState,
};
use deploy_db::DbStore;
use crate::auth::ServerAuth;
use deploy_auth::{
    now_unix_ms, signing_payload, verify_rpc_metadata, verify_upload_metadata, META_PROJECT,
    META_VERSION,
};
use std::collections::HashMap;
use deploy_proto::deploy::{
    deploy_service_server::DeployService, DeployChunk, DeployResponse, PairRequest, PairResponse,
    RestartProcessRequest, RollbackRequest, RollbackResponse, StatusRequest, StatusResponse,
    StopProcessRequest,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::auth::{sign_pair_response, verify_pair_signature};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
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
    pub binary_fallback: String,
    pub states: Arc<tokio::sync::Mutex<HashMap<String, AppState>>>,
    pub db: Option<Arc<DbStore>>,
    pub auth: Option<Arc<ServerAuth>>,
}

impl DeployServiceImpl {
    pub fn new(
        base_root: PathBuf,
        max_upload_bytes: u64,
        binary_fallback: String,
        states: Arc<tokio::sync::Mutex<HashMap<String, AppState>>>,
        db: Option<Arc<DbStore>>,
        auth: Option<Arc<ServerAuth>>,
    ) -> Self {
        Self {
            base_root,
            max_upload_bytes,
            binary_fallback,
            states,
            db,
            auth,
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

        Ok(Response::new(StatusResponse {
            current_version: current,
            state: st.state.clone(),
        }))
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

        Ok(Response::new(StatusResponse {
            current_version: current,
            state: "stopped".to_string(),
        }))
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
        Ok(Response::new(StatusResponse {
            current_version: st.current_version.clone(),
            state: st.state.clone(),
        }))
    }
}
