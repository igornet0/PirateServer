use deploy_auth::{
    attach_auth_metadata, attach_auth_metadata_upload_server_stack,
    insert_stack_apply_sha256_metadata,
};
use deploy_proto::deploy::{
    DeployChunk, DeployResponse, ServerStackInfo, ServerStackResponse, StackApplyOptions,
};
use prost::Message;
use serde::Serialize;
use std::sync::Arc;
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use futures_util::stream::poll_fn;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::task::Poll;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tonic::Request;

use crate::ops::{build_chunks_with_manifest, build_server_stack_chunks};
use deploy_core::pirate_project::PirateManifest;

/// Result of packing + upload (for CLI / desktop metrics).
pub struct DeploySummary {
    pub response: DeployResponse,
    pub artifact_bytes: u64,
    pub chunk_count: usize,
}

/// Packed project directory (`.tar.gz`) ready for gRPC or HTTP upload.
#[derive(Debug)]
pub struct PackedDeployArtifact {
    pub path: PathBuf,
    pub artifact_bytes: u64,
    pub manifest_toml: Option<String>,
    pub chunk_count: usize,
}

/// Progress snapshot for desktop UI during [`deploy_directory_with_progress`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployProgressEvent {
    /// One of: `prepare`, `archive`, `upload`, `apply`.
    pub phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_total: Option<u64>,
    /// Optional sub-status for the UI (chunk retries, finalize, resumable session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl DeployProgressEvent {
    pub fn phase_only(phase: &'static str) -> Self {
        Self {
            phase,
            upload_sent: None,
            upload_total: None,
            detail: None,
        }
    }

    pub fn upload_bytes(sent: u64, total: u64) -> Self {
        Self {
            phase: "upload",
            upload_sent: Some(sent),
            upload_total: Some(total),
            detail: None,
        }
    }

    pub fn upload_bytes_detail(sent: u64, total: u64, detail: impl Into<String>) -> Self {
        Self {
            phase: "upload",
            upload_sent: Some(sent),
            upload_total: Some(total),
            detail: Some(detail.into()),
        }
    }
}

/// Best-effort cancel for in-flight artifact upload (checked between chunks).
static ARTIFACT_UPLOAD_CANCEL: AtomicBool = AtomicBool::new(false);
/// Best-effort cancel for in-flight server-stack upload (checked between chunks).
static SERVER_STACK_UPLOAD_CANCEL: AtomicBool = AtomicBool::new(false);

/// Request cancellation of artifact upload streaming (deploy). Reset when a new upload starts.
pub fn set_artifact_upload_cancel(v: bool) {
    ARTIFACT_UPLOAD_CANCEL.store(v, Ordering::SeqCst);
}

/// Request cancellation of server-stack upload streaming. Reset when a new upload starts.
pub fn set_server_stack_upload_cancel(v: bool) {
    SERVER_STACK_UPLOAD_CANCEL.store(v, Ordering::SeqCst);
}

/// Stream a packed artifact to deploy-server. When `signing_key` is set, request is authenticated.
pub async fn upload_artifact(
    endpoint: &str,
    artifact: &[u8],
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
) -> Result<DeployResponse, tonic::Status> {
    upload_artifact_with_manifest(
        endpoint,
        artifact,
        version,
        project_id,
        chunk_size,
        signing_key,
        None,
        "tar_gz",
    )
    .await
}

/// Same as [`upload_artifact`] with optional `pirate.toml` body on last chunk (UTF-8).
pub async fn upload_artifact_with_manifest(
    endpoint: &str,
    artifact: &[u8],
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    manifest_toml: Option<&str>,
    artifact_kind: &str,
) -> Result<DeployResponse, tonic::Status> {
    let digest = Sha256::digest(artifact);
    let sha256_hex = hex::encode(digest);
    ARTIFACT_UPLOAD_CANCEL.store(false, Ordering::SeqCst);
    let chunks = build_chunks_with_manifest(
        artifact,
        version,
        project_id,
        &sha256_hex,
        chunk_size,
        manifest_toml,
        artifact_kind,
    );

    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let mut chunk_iter = chunks.into_iter();
    let st = poll_fn(move |_cx| {
        if ARTIFACT_UPLOAD_CANCEL.load(Ordering::SeqCst) {
            return Poll::Ready(None);
        }
        match chunk_iter.next() {
            Some(c) => Poll::Ready(Some(c)),
            None => Poll::Ready(None),
        }
    });
    let mut req = Request::new(Box::pin(st));
    if let Some(sk) = signing_key {
        attach_auth_metadata(&mut req, sk, "Upload", project_id, version)
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
    }

    let resp = client.upload(req).await?.into_inner();
    Ok(resp)
}

/// Stream `reader` to deploy-server (`Upload`). `total_len` when known (e.g. file size); `None` for HTTP bodies without length.
pub async fn upload_artifact_from_async_read<R>(
    endpoint: &str,
    reader: R,
    total_len: Option<u64>,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    manifest_toml: Option<&str>,
    artifact_kind: &str,
) -> Result<DeployResponse, tonic::Status>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    upload_artifact_from_async_read_impl(
        endpoint,
        reader,
        total_len,
        version,
        project_id,
        chunk_size,
        signing_key,
        manifest_toml,
        artifact_kind,
        None,
    )
    .await
}

/// Same as [`upload_artifact_from_async_read`] but reports bytes read vs total for known-length bodies.
pub async fn upload_artifact_from_async_read_with_upload_progress<R, F>(
    endpoint: &str,
    reader: R,
    total_len: Option<u64>,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    manifest_toml: Option<&str>,
    artifact_kind: &str,
    on_upload: F,
) -> Result<DeployResponse, tonic::Status>
where
    R: AsyncRead + Unpin + Send + 'static,
    F: FnMut(u64, u64) + Send + 'static,
{
    upload_artifact_from_async_read_impl(
        endpoint,
        reader,
        total_len,
        version,
        project_id,
        chunk_size,
        signing_key,
        manifest_toml,
        artifact_kind,
        Some(Box::new(on_upload)),
    )
    .await
}

type UploadProgressBox = Box<dyn FnMut(u64, u64) + Send>;

async fn upload_artifact_from_async_read_impl<R>(
    endpoint: &str,
    mut reader: R,
    total_len: Option<u64>,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    manifest_toml: Option<&str>,
    artifact_kind: &str,
    upload_progress: Option<UploadProgressBox>,
) -> Result<DeployResponse, tonic::Status>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let cs = chunk_size.max(1);
    let version_owned = version.to_string();
    let project_id_owned = project_id.to_string();
    let man = manifest_toml.unwrap_or("").to_string();
    let kind = artifact_kind.to_string();
    ARTIFACT_UPLOAD_CANCEL.store(false, Ordering::SeqCst);

    let progress_hook = upload_progress.map(|b| Arc::new(Mutex::new(b)));

    let stream = async_stream::stream! {
        let mut hasher = Sha256::new();
        let mut read_total = 0u64;
        let mut first = true;
        let mut buf = vec![0u8; cs];

        if let Some(0) = total_len {
            let sha256_hex = hex::encode(hasher.finalize());
            yield DeployChunk {
                data: vec![],
                version: version_owned.clone(),
                is_last: true,
                sha256_hex,
                project_id: project_id_owned.clone(),
                manifest_toml: man.clone(),
                artifact_kind: kind.clone(),
            };
            return;
        }

        if let Some(expected) = total_len {
            loop {
                if ARTIFACT_UPLOAD_CANCEL.load(Ordering::SeqCst) {
                    return;
                }
                let n = match reader.read(&mut buf).await {
                    Ok(n) => n,
                    Err(_) => return,
                };
                if n == 0 {
                    return;
                }
                hasher.update(&buf[..n]);
                read_total += n as u64;
                if let Some(h) = progress_hook.as_ref() {
                    if let Ok(mut f) = h.lock() {
                        (f)(read_total, expected);
                    }
                }
                let is_last = read_total == expected;
                let sha256_hex = if is_last {
                    hex::encode(hasher.clone().finalize())
                } else {
                    String::new()
                };
                yield DeployChunk {
                    data: buf[..n].to_vec(),
                    version: if first { version_owned.clone() } else { String::new() },
                    is_last,
                    sha256_hex,
                    project_id: if first { project_id_owned.clone() } else { String::new() },
                    manifest_toml: if is_last { man.clone() } else { String::new() },
                    artifact_kind: if is_last { kind.clone() } else { String::new() },
                };
                first = false;
                if is_last {
                    break;
                }
            }
            return;
        }

        // Unknown length: one-byte lookahead so the last non-empty chunk carries `is_last` + digest.
        let mut prev: Option<Vec<u8>> = None;
        loop {
            if ARTIFACT_UPLOAD_CANCEL.load(Ordering::SeqCst) {
                return;
            }
            let n = match reader.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            read_total += n as u64;
            if let Some(h) = progress_hook.as_ref() {
                if let Ok(mut f) = h.lock() {
                    // Total unknown: report monotonic byte count only.
                    (f)(read_total, 0);
                }
            }
            let chunk_bytes = buf[..n].to_vec();
            if let Some(p) = prev.take() {
                yield DeployChunk {
                    data: p,
                    version: if first { version_owned.clone() } else { String::new() },
                    is_last: false,
                    sha256_hex: String::new(),
                    project_id: if first { project_id_owned.clone() } else { String::new() },
                    manifest_toml: String::new(),
                    artifact_kind: String::new(),
                };
                first = false;
            }
            prev = Some(chunk_bytes);
        }
        let sha256_hex = hex::encode(hasher.finalize());
        if let Some(p) = prev {
            yield DeployChunk {
                data: p,
                version: if first { version_owned.clone() } else { String::new() },
                is_last: true,
                sha256_hex,
                project_id: if first { project_id_owned.clone() } else { String::new() },
                manifest_toml: man,
                artifact_kind: kind,
            };
        } else {
            yield DeployChunk {
                data: vec![],
                version: version_owned,
                is_last: true,
                sha256_hex,
                project_id: project_id_owned,
                manifest_toml: man,
                artifact_kind: kind,
            };
        }
    };

    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let mut req = Request::new(Box::pin(stream));
    if let Some(sk) = signing_key {
        attach_auth_metadata(&mut req, sk, "Upload", project_id, version)
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
    }
    Ok(client.upload(req).await?.into_inner())
}

/// Pack `dir` into a temporary `.tar.gz` (same rules as [`deploy_directory_with_progress`]).
/// Caller must delete [`PackedDeployArtifact::path`] after upload succeeds or fails.
///
/// Pass the same `Arc<Mutex<F>>` as [`upload_packed_tar_gz_grpc`] so progress callbacks stay unified.
pub async fn pack_directory_for_deploy<F>(
    dir: &Path,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    on_event: &Arc<Mutex<F>>,
) -> Result<PackedDeployArtifact, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let emit = |ev: DeployProgressEvent| {
        if let Ok(mut f) = on_event.lock() {
            (f)(ev);
        }
    };

    emit(DeployProgressEvent::phase_only("prepare"));

    let dir = dir.canonicalize().map_err(|e| {
        format!("cannot resolve {}: {e}", dir.display())
    })?;
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    crate::ops::validate_version(version).map_err(|e| e.to_string())?;
    deploy_core::validate_project_id(project_id).map_err(|e| e.to_string())?;
    let manifest_path = dir.join("pirate.toml");
    let manifest_toml = std::fs::read_to_string(&manifest_path).ok();
    let release_outputs = match PirateManifest::read_file(&manifest_path) {
        Ok(m) => m.release_output_paths(),
        Err(_) => Vec::new(),
    };
    if release_outputs.is_empty() {
        eprintln!(
            "warning: [build].output_path(s) is not set; fallback to project root packaging with .pirateignore filtering"
        );
    }

    emit(DeployProgressEvent::phase_only("archive"));

    let tmp_path = std::env::temp_dir().join(format!(
        "pirate-deploy-{}-{}.tar.gz",
        std::process::id(),
        rand::random::<u64>()
    ));
    let pack_dst = tmp_path.clone();
    let dir_for_pack = dir.clone();
    let outputs = release_outputs.clone();
    let ignore = dir.join(".pirateignore");
    tokio::task::spawn_blocking(move || {
        crate::ops::pack_release_sources_to_path(&dir_for_pack, &outputs, Some(&ignore), &pack_dst)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let meta = tokio::fs::metadata(&tmp_path)
        .await
        .map_err(|e| e.to_string())?;
    let artifact_bytes = meta.len();
    let cs = chunk_size.max(1);
    let chunk_count = ((artifact_bytes as usize) + cs - 1) / cs;

    Ok(PackedDeployArtifact {
        path: tmp_path,
        artifact_bytes,
        manifest_toml,
        chunk_count,
    })
}

/// Stream a packed `.tar.gz` to deploy-server over gRPC (`Upload`). Does **not** delete `packed.path`.
pub async fn upload_packed_tar_gz_grpc<F>(
    endpoint: &str,
    packed: &PackedDeployArtifact,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    on_event: &Arc<Mutex<F>>,
) -> Result<DeploySummary, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let emit = |ev: DeployProgressEvent| {
        if let Ok(mut f) = on_event.lock() {
            (f)(ev);
        }
    };

    emit(DeployProgressEvent::phase_only("upload"));

    let file = tokio::fs::File::open(&packed.path)
        .await
        .map_err(|e| e.to_string())?;

    let on_ev = Arc::clone(on_event);
    let upload_result = upload_artifact_from_async_read_with_upload_progress(
        endpoint,
        file,
        Some(packed.artifact_bytes),
        version,
        project_id,
        chunk_size,
        signing_key,
        packed.manifest_toml.as_deref(),
        "tar_gz",
        move |sent, total| {
            if let Ok(mut f) = on_ev.lock() {
                (f)(DeployProgressEvent::upload_bytes(sent, total));
            }
        },
    )
    .await
    .map_err(|s| {
        let m = s.message();
        if m.is_empty() {
            format!("{s:?}")
        } else {
            m.to_string()
        }
    });
    let response = upload_result?;
    emit(DeployProgressEvent::phase_only("apply"));
    Ok(DeploySummary {
        response,
        artifact_bytes: packed.artifact_bytes,
        chunk_count: packed.chunk_count,
    })
}

/// Pack directory, hash, upload. `dir` must exist and be a directory.
/// If `pirate.toml` exists in `dir`, its contents are sent as `manifest_toml` on the last chunk.
pub async fn deploy_directory(
    endpoint: &str,
    dir: &Path,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
) -> Result<DeploySummary, String> {
    deploy_directory_with_progress(
        endpoint,
        dir,
        version,
        project_id,
        chunk_size,
        signing_key,
        |_ev: DeployProgressEvent| {},
    )
    .await
}

/// Same as [`deploy_directory`] but invokes `on_event` for each pipeline stage and during upload (bytes read).
pub async fn deploy_directory_with_progress<F>(
    endpoint: &str,
    dir: &Path,
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    on_event: F,
) -> Result<DeploySummary, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let on_event = Arc::new(Mutex::new(on_event));

    let emit = |ev: DeployProgressEvent| {
        if let Ok(mut f) = on_event.lock() {
            (f)(ev);
        }
    };

    emit(DeployProgressEvent::phase_only("prepare"));

    let dir = dir.canonicalize().map_err(|e| {
        format!("cannot resolve {}: {e}", dir.display())
    })?;
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    crate::ops::validate_version(version).map_err(|e| e.to_string())?;
    deploy_core::validate_project_id(project_id).map_err(|e| e.to_string())?;
    let manifest_path = dir.join("pirate.toml");
    let manifest_toml = std::fs::read_to_string(&manifest_path).ok();
    let release_outputs = match PirateManifest::read_file(&manifest_path) {
        Ok(m) => m.release_output_paths(),
        Err(_) => Vec::new(),
    };
    if release_outputs.is_empty() {
        eprintln!(
            "warning: [build].output_path(s) is not set; fallback to project root packaging with .pirateignore filtering"
        );
    }

    emit(DeployProgressEvent::phase_only("archive"));

    let tmp_path = std::env::temp_dir().join(format!(
        "pirate-deploy-{}-{}.tar.gz",
        std::process::id(),
        rand::random::<u64>()
    ));
    let pack_dst = tmp_path.clone();
    let dir_for_pack = dir.clone();
    let outputs = release_outputs.clone();
    let ignore = dir.join(".pirateignore");
    tokio::task::spawn_blocking(move || {
        crate::ops::pack_release_sources_to_path(&dir_for_pack, &outputs, Some(&ignore), &pack_dst)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let meta = tokio::fs::metadata(&tmp_path)
        .await
        .map_err(|e| e.to_string())?;
    let artifact_bytes = meta.len();
    let cs = chunk_size.max(1);
    let chunk_count = ((artifact_bytes as usize) + cs - 1) / cs;

    emit(DeployProgressEvent::phase_only("upload"));

    let file = tokio::fs::File::open(&tmp_path)
        .await
        .map_err(|e| e.to_string())?;

    let on_ev = on_event.clone();
    let upload_result = upload_artifact_from_async_read_with_upload_progress(
        endpoint,
        file,
        Some(artifact_bytes),
        version,
        project_id,
        chunk_size,
        signing_key,
        manifest_toml.as_deref(),
        "tar_gz",
        move |sent, total| {
            if let Ok(mut f) = on_ev.lock() {
                (f)(DeployProgressEvent::upload_bytes(sent, total));
            }
        },
    )
    .await
    .map_err(|s| {
        let m = s.message();
        if m.is_empty() {
            format!("{s:?}")
        } else {
            m.to_string()
        }
    });
    let _ = tokio::fs::remove_file(&tmp_path).await;
    let response = upload_result?;
    emit(DeployProgressEvent::phase_only("apply"));
    Ok(DeploySummary {
        response,
        artifact_bytes,
        chunk_count,
    })
}

/// Stream server-stack tarball to deploy-server (`UploadServerStack`).
pub async fn upload_server_stack_artifact(
    endpoint: &str,
    artifact: &[u8],
    version: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    apply_options: Option<StackApplyOptions>,
) -> Result<ServerStackResponse, tonic::Status> {
    upload_server_stack_artifact_with_progress(
        endpoint,
        artifact,
        version,
        chunk_size,
        signing_key,
        apply_options,
        |_, _| {},
    )
    .await
}

/// Same as [`upload_server_stack_artifact`] but reports upload progress as bytes sent / total.
pub async fn upload_server_stack_artifact_with_progress<F>(
    endpoint: &str,
    artifact: &[u8],
    version: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
    apply_options: Option<StackApplyOptions>,
    on_progress: F,
) -> Result<ServerStackResponse, tonic::Status>
where
    F: FnMut(u64, u64) + Send + 'static,
{
    let digest = Sha256::digest(artifact);
    let sha256_hex = hex::encode(digest);
    let stack_apply_sha_hex: Option<String> = apply_options.as_ref().map(|o| {
        let mut buf = Vec::new();
        o.encode(&mut buf).unwrap_or_default();
        hex::encode(Sha256::digest(&buf))
    });
    let chunks = build_server_stack_chunks(
        artifact,
        version,
        &sha256_hex,
        chunk_size,
        apply_options,
    );
    let total = artifact.len() as u64;
    let mut chunk_iter = chunks.into_iter();
    let mut sent = 0u64;
    let on_progress = Mutex::new(on_progress);

    SERVER_STACK_UPLOAD_CANCEL.store(false, Ordering::SeqCst);
    let st = poll_fn(move |_cx| {
        if SERVER_STACK_UPLOAD_CANCEL.load(Ordering::SeqCst) {
            return Poll::Ready(None);
        }
        let ch = match chunk_iter.next() {
            Some(c) => c,
            None => return Poll::Ready(None),
        };
        sent += ch.data.len() as u64;
        if let Ok(mut cb) = on_progress.lock() {
            (cb)(sent, total);
        }
        Poll::Ready(Some(ch))
    });

    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let mut req = Request::new(Box::pin(st));
    if let Some(sk) = signing_key {
        attach_auth_metadata_upload_server_stack(
            &mut req,
            sk,
            version,
            stack_apply_sha_hex.as_deref(),
        )
        .map_err(|e| tonic::Status::internal(e.to_string()))?;
    } else {
        insert_stack_apply_sha256_metadata(&mut req, stack_apply_sha_hex.as_deref())
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
    }

    let resp = client.upload_server_stack(req).await?.into_inner();
    Ok(resp)
}

pub async fn fetch_server_stack_info(
    endpoint: &str,
    signing_key: Option<&SigningKey>,
) -> Result<ServerStackInfo, tonic::Status> {
    use deploy_proto::deploy::ServerStackInfoRequest;
    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let mut req = Request::new(ServerStackInfoRequest {});
    if let Some(sk) = signing_key {
        attach_auth_metadata(&mut req, sk, "GetServerStackInfo", "", "")
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
    }
    Ok(client.get_server_stack_info(req).await?.into_inner())
}
