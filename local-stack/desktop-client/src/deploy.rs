//! gRPC deploy / rollback from the desktop (reuse `deploy-client` packing + upload).

use crate::connection::{load_endpoint, load_project_id, load_signing_key_for_endpoint};
use deploy_auth::attach_auth_metadata;
use deploy_client::{deploy_directory, validate_version_label};
use deploy_proto::deploy::RollbackRequest;
use deploy_proto::DeployServiceClient;
use serde::Serialize;
use std::path::PathBuf;
use tonic::Request;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployOutcome {
    pub status: String,
    pub deployed_version: String,
    pub artifact_bytes: u64,
    pub chunk_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackOutcome {
    pub status: String,
    pub active_version: String,
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// Pack `dir` and upload to the saved gRPC endpoint (same as `client deploy`).
pub fn run_deploy(dir: PathBuf, version: String, chunk_size: usize) -> Result<DeployOutcome, String> {
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    validate_version_label(&version).map_err(|e| e.to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let rt = runtime()?;
    let project = load_project_id();
    let resp = rt.block_on(deploy_directory(
        &endpoint,
        &dir,
        &version,
        &project,
        chunk_size,
        sk.as_ref(),
    ))?;
    Ok(DeployOutcome {
        status: resp.response.status,
        deployed_version: resp.response.deployed_version,
        artifact_bytes: resp.artifact_bytes,
        chunk_count: resp.chunk_count,
    })
}

/// Roll back to an existing release on the saved server.
pub fn run_rollback(version: String) -> Result<RollbackOutcome, String> {
    validate_version_label(&version).map_err(|e| e.to_string())?;
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    let project = load_project_id();
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let rt = runtime()?;
    let inner = rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let mut req = Request::new(RollbackRequest {
            version: version.clone(),
            project_id: project.clone(),
        });
        if let Some(ref key) = sk {
            attach_auth_metadata(&mut req, key, "Rollback", &project, &version)
                .map_err(|e| e.to_string())?;
        }
        client
            .rollback(req)
            .await
            .map_err(|e| format!("Rollback failed: {e}"))
            .map(|r| r.into_inner())
    })?;
    Ok(RollbackOutcome {
        status: inner.status,
        active_version: inner.active_version,
    })
}

