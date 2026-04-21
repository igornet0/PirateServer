//! Stream a packed tarball from disk to deploy-server (`Upload` RPC).

use crate::ControlError;
use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::deploy_service_client::DeployServiceClient;
use deploy_proto::deploy::{DeployChunk, DeployResponse};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tonic::Request;

/// gRPC `Upload` of `artifact_path` (expected size `file_len`). Enforces `max_upload_bytes` before streaming.
pub async fn grpc_upload_project_artifact_from_path(
    grpc_endpoint: &str,
    grpc_signing_key: Option<&Arc<SigningKey>>,
    project_id: &str,
    version: &str,
    manifest_toml: Option<&str>,
    artifact_path: &Path,
    file_len: u64,
    chunk_size: usize,
    max_upload_bytes: u64,
) -> Result<DeployResponse, ControlError> {
    if file_len > max_upload_bytes {
        return Err(ControlError::Grpc(format!(
            "artifact exceeds limit of {} bytes",
            max_upload_bytes
        )));
    }
    let cs = chunk_size.max(1);
    let version_owned = version.to_string();
    let project_id_owned = project_id.to_string();
    let man = manifest_toml.unwrap_or("").to_string();
    let kind = "tar_gz".to_string();
    let path_owned = artifact_path.to_path_buf();

    let stream = async_stream::stream! {
        let mut file = match tokio::fs::File::open(&path_owned).await {
            Ok(f) => f,
            Err(_) => return,
        };
        let mut hasher = Sha256::new();
        let mut read_total = 0u64;
        let mut first = true;
        let mut buf = vec![0u8; cs];

        if file_len == 0 {
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

        loop {
            let n = match file.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if n == 0 {
                return;
            }
            hasher.update(&buf[..n]);
            read_total += n as u64;
            let is_last = read_total == file_len;
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
    };

    let mut client = DeployServiceClient::connect(grpc_endpoint.to_string())
        .await
        .map_err(|e| ControlError::Grpc(e.to_string()))?;
    let mut req = Request::new(Box::pin(stream));
    if let Some(sk) = grpc_signing_key {
        attach_auth_metadata(&mut req, sk.as_ref(), "Upload", project_id, version)
            .map_err(|e| ControlError::Grpc(e.to_string()))?;
    }
    let resp = client
        .upload(req)
        .await
        .map_err(|e| ControlError::Grpc(e.message().to_string()))?;
    Ok(resp.into_inner())
}
