use deploy_auth::{
    attach_auth_metadata, attach_auth_metadata_upload_server_stack,
    insert_stack_apply_sha256_metadata,
};
use deploy_proto::deploy::{DeployResponse, ServerStackInfo, ServerStackResponse, StackApplyOptions};
use prost::Message;

/// Result of packing + upload (for CLI / desktop metrics).
pub struct DeploySummary {
    pub response: DeployResponse,
    pub artifact_bytes: u64,
    pub chunk_count: usize,
}
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use futures_util::stream::poll_fn;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::task::Poll;
use tonic::Request;

use crate::ops::{build_chunks_with_manifest, build_server_stack_chunks};
use deploy_core::pirate_project::PirateManifest;

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
    let artifact = crate::ops::pack_release_sources(
        &dir,
        &release_outputs,
        Some(&dir.join(".pirateignore")),
    )
    .map_err(|e| e.to_string())?;
    let artifact_bytes = artifact.len() as u64;
    let cs = chunk_size.max(1);
    let chunk_count = (artifact.len() + cs - 1) / cs;
    let response = upload_artifact_with_manifest(
        endpoint,
        &artifact,
        version,
        project_id,
        chunk_size,
        signing_key,
        manifest_toml.as_deref(),
        "tar_gz",
    )
    .await
    .map_err(|s| {
        let m = s.message();
        if m.is_empty() {
            format!("{s:?}")
        } else {
            m.to_string()
        }
    })?;
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
