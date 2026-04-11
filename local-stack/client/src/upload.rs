use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::DeployResponse;

/// Result of packing + upload (for CLI / desktop metrics).
pub struct DeploySummary {
    pub response: DeployResponse,
    pub artifact_bytes: u64,
    pub chunk_count: usize,
}
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use futures_util::stream;
use sha2::{Digest, Sha256};
use std::path::Path;
use tonic::Request;

use crate::ops::build_chunks;

/// Stream a packed artifact to deploy-server. When `signing_key` is set, request is authenticated.
pub async fn upload_artifact(
    endpoint: &str,
    artifact: &[u8],
    version: &str,
    project_id: &str,
    chunk_size: usize,
    signing_key: Option<&SigningKey>,
) -> Result<DeployResponse, tonic::Status> {
    let digest = Sha256::digest(artifact);
    let sha256_hex = hex::encode(digest);
    let chunks = build_chunks(artifact, version, project_id, &sha256_hex, chunk_size);

    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| tonic::Status::unavailable(e.to_string()))?;
    let stream = stream::iter(chunks);
    let mut req = Request::new(stream);
    if let Some(sk) = signing_key {
        attach_auth_metadata(&mut req, sk, "Upload", project_id, version)
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
    }

    let resp = client.upload(req).await?.into_inner();
    Ok(resp)
}

/// Pack directory, hash, upload. `dir` must exist and be a directory.
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
    let artifact = crate::ops::pack_directory(&dir).map_err(|e| e.to_string())?;
    let artifact_bytes = artifact.len() as u64;
    let cs = chunk_size.max(1);
    let chunk_count = (artifact.len() + cs - 1) / cs;
    let response = upload_artifact(endpoint, &artifact, version, project_id, chunk_size, signing_key)
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
