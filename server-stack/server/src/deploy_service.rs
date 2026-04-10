//! Deploy: stream → temp file, SHA-256, tar unpack, symlink, process control.

use deploy_core::{
    read_current_version_from_symlink, refresh_process_state, release_dir_for_version,
    validate_version as validate_version_core, AppState,
};
use deploy_db::DbStore;
use deploy_proto::deploy::{
    deploy_service_server::DeployService, DeployChunk, DeployResponse, RollbackRequest,
    RollbackResponse, StatusRequest, StatusResponse,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tonic::{Request, Response, Status, Streaming};
use tracing::{error, info, warn};

fn validate_version(version: &str) -> Result<(), Status> {
    validate_version_core(version).map_err(|e| Status::invalid_argument(e))
}

#[derive(Clone)]
pub struct DeployServiceImpl {
    pub root: PathBuf,
    pub max_upload_bytes: u64,
    pub binary_fallback: String,
    pub state: Arc<tokio::sync::Mutex<AppState>>,
    pub db: Option<Arc<DbStore>>,
}

impl DeployServiceImpl {
    pub fn new(
        root: PathBuf,
        max_upload_bytes: u64,
        binary_fallback: String,
        state: Arc<tokio::sync::Mutex<AppState>>,
        db: Option<Arc<DbStore>>,
    ) -> Self {
        Self {
            root,
            max_upload_bytes,
            binary_fallback,
            state,
            db,
        }
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join(".staging")
    }

    fn spawn_db_record(
        &self,
        kind: &'static str,
        deployed_version: &str,
        current_version: &str,
        state: &str,
        last_error: Option<&str>,
    ) {
        let Some(db) = self.db.clone() else {
            return;
        };
        let deployed_version = deployed_version.to_string();
        let current_version = current_version.to_string();
        let state = state.to_string();
        let last_err = last_error.map(|s| s.to_string());
        let snapshot = format!("{state}|{current_version}|{:?}", last_err);
        tokio::spawn(async move {
            if let Err(e) = db
                .record_event(kind, &deployed_version, Some(&snapshot))
                .await
            {
                error!(%e, "deploy_db record_event");
            }
            if let Err(e) = db
                .upsert_snapshot(&current_version, &state, last_err.as_deref())
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
    async fn upload(
        &self,
        request: Request<Streaming<DeployChunk>>,
    ) -> Result<Response<DeployResponse>, Status> {
        let mut stream = request.into_inner();

        let mut version: Option<String> = None;
        let mut hasher = Sha256::new();
        let mut written: u64 = 0;
        let mut expected_sha_hex: Option<String> = None;

        tokio::fs::create_dir_all(&self.staging_dir())
            .await
            .map_err(|e| Status::internal(format!("staging dir: {e}")))?;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let temp_path = self.staging_dir().join(format!("upload_{stamp}.tar.gz"));

        {
            let mut file = tokio::fs::File::create(&temp_path)
                .await
                .map_err(|e| Status::internal(format!("temp file: {e}")))?;

            while let Some(item) = stream.next().await {
                let chunk = item.map_err(|e| Status::internal(e.to_string()))?;

                if version.is_none() {
                    if chunk.version.is_empty() {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        return Err(Status::invalid_argument("first chunk must set version"));
                    }
                    validate_version(&chunk.version)?;
                    version = Some(chunk.version.clone());
                } else if !chunk.version.is_empty()
                    && chunk.version.as_str() != version.as_deref().unwrap()
                {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(Status::invalid_argument("version mismatch between chunks"));
                }

                let n = chunk.data.len() as u64;
                if written.saturating_add(n) > self.max_upload_bytes {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(Status::resource_exhausted(format!(
                        "artifact exceeds limit of {} bytes",
                        self.max_upload_bytes
                    )));
                }

                hasher.update(&chunk.data);
                file.write_all(&chunk.data)
                    .await
                    .map_err(|e| Status::internal(format!("write: {e}")))?;
                written = written.saturating_add(n);

                if chunk.is_last {
                    if chunk.sha256_hex.is_empty() {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        return Err(Status::invalid_argument(
                            "sha256_hex required when is_last is true",
                        ));
                    }
                    expected_sha_hex = Some(chunk.sha256_hex.clone());
                }
            }
        }

        let version = version.ok_or_else(|| {
            let _ = std::fs::remove_file(&temp_path);
            Status::invalid_argument("no version in stream")
        })?;

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

        let release_dir = release_dir_for_version(&self.root, &version);
        tokio::fs::create_dir_all(deploy_core::releases_dir(&self.root))
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

        info!(version = %version, "artifact unpacked");

        let root = self.root.clone();
        let bf = self.binary_fallback.clone();
        let mut st = self.state.lock().await;

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
                info!(version = %version, "deployed and started");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(st);
                self.spawn_db_record("upload", &version, &cur, &state, err.as_deref());
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
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let mut st = self.state.lock().await;
        refresh_process_state(&mut st);

        let mut current = st.current_version.clone();
        if current.is_empty() {
            if let Some(v) = read_current_version_from_symlink(&self.root) {
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
        let v = request.into_inner().version;
        validate_version(&v)?;

        let target = release_dir_for_version(&self.root, &v);
        if !target.is_dir() {
            return Err(Status::not_found(format!("release {v} not found")));
        }

        let root = self.root.clone();
        let bf = self.binary_fallback.clone();
        let mut st = self.state.lock().await;

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
                info!(version = %v, "rollback complete");
                let cur = st.current_version.clone();
                let state = st.state.clone();
                let err = st.last_error.clone();
                drop(st);
                self.spawn_db_record("rollback", &v, &cur, &state, err.as_deref());
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
}
