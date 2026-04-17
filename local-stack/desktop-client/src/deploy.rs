//! gRPC deploy / rollback from the desktop (reuse `deploy-client` packing + upload).

use crate::connection::{load_endpoint, load_project_id, load_signing_key_for_endpoint};
use deploy_auth::attach_auth_metadata;
use deploy_client::{
    deploy_directory, detect_services, generate_proxy_config, validate_version_label,
    DeployValidationReport, ServiceDetectionReport,
};
use deploy_core::pirate_project::PirateManifest;

use crate::host_services_compat::{summarize_host_services_for_manifest, HostServicesCompatSummary};
use deploy_proto::deploy::{RemoveProjectRequest, RollbackRequest, ValidateDeployRequest};
use deploy_proto::DeployServiceClient;
use serde::Serialize;
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectOutcome {
    pub status: String,
    pub project_id: String,
    pub removed_root: String,
    pub removed_db_rows: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeployCheck {
    pub project_id: String,
    pub uploaded: bool,
    pub current_version: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAccessAnalysis {
    pub project_id: String,
    pub detection: ServiceDetectionReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nginx_preview: Option<String>,
    /// Required host package ids from manifest vs `GET /api/v1/host-services` when possible.
    pub host_services: HostServicesCompatSummary,
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, String> {
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

pub fn resolve_project_id_from_manifest(project_dir: &Path) -> Result<String, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let pid = m.project.deploy_target_project_id();
    deploy_core::validate_project_id(&pid).map_err(|e| e.to_string())?;
    Ok(pid)
}

pub fn read_release_version_from_manifest(project_dir: &Path) -> Result<String, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let v = m.project.version.trim();
    if v.is_empty() {
        return Err(format!(
            "{}: [project].version is empty",
            manifest_path.display()
        ));
    }
    validate_version_label(v).map_err(|e| e.to_string())?;
    Ok(v.to_string())
}

pub fn check_project_uploaded(project_dir: PathBuf) -> Result<ProjectDeployCheck, String> {
    let pid = resolve_project_id_from_manifest(&project_dir)?;
    let status = crate::connection::verify_grpc_status_for_project(&pid)?;
    let current = status.current_version.trim().to_string();
    let uploaded = !current.is_empty() && !current.starts_with("stack@");
    Ok(ProjectDeployCheck {
        project_id: pid,
        uploaded,
        current_version: current,
        state: status.state,
    })
}

pub fn run_remove_project(project_id: String) -> Result<RemoveProjectOutcome, String> {
    deploy_core::validate_project_id(&project_id).map_err(|e| e.to_string())?;
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let rt = runtime()?;
    let pid = deploy_core::normalize_project_id(&project_id);
    let inner = rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let mut req = Request::new(RemoveProjectRequest {
            project_id: pid.clone(),
        });
        if let Some(ref key) = sk {
            attach_auth_metadata(&mut req, key, "RemoveProject", &pid, "")
                .map_err(|e| e.to_string())?;
        }
        client
            .remove_project(req)
            .await
            .map_err(|e| format!("RemoveProject failed: {e}"))
            .map(|r| r.into_inner())
    })?;
    Ok(RemoveProjectOutcome {
        status: inner.status,
        project_id: inner.project_id,
        removed_root: inner.removed_root,
        removed_db_rows: inner.removed_db_rows,
    })
}

pub fn analyze_network_access(project_dir: PathBuf) -> Result<NetworkAccessAnalysis, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let mut m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let detection = detect_services(&project_dir, Some(&m));
    deploy_client::apply_detected_services_to_manifest(&mut m, &detection);
    let server_name = if m.network.access.domain.trim().is_empty() {
        "_"
    } else {
        m.network.access.domain.trim()
    };
    let nginx_preview = generate_proxy_config(&m, server_name).ok();
    let host_services = summarize_host_services_for_manifest(&m);
    Ok(NetworkAccessAnalysis {
        project_id: m.project.deploy_target_project_id(),
        detection,
        nginx_preview,
        host_services,
    })
}

pub fn validate_network_access_remote(project_dir: PathBuf) -> Result<DeployValidationReport, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest_toml = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let project_id = m.project.deploy_target_project_id();
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let rt = runtime()?;
    let report = rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let mut req = Request::new(ValidateDeployRequest {
            project_id: project_id.clone(),
            manifest_toml,
        });
        if let Some(ref key) = sk {
            attach_auth_metadata(&mut req, key, "ValidateDeploy", &project_id, "")
                .map_err(|e| e.to_string())?;
        }
        client
            .validate_deploy(req)
            .await
            .map_err(|e| format!("ValidateDeploy failed: {e}"))
            .map(|r| r.into_inner())
    })?;
    Ok(DeployValidationReport {
        allow: report.allow,
        blockers: report.blockers,
        warnings: report.warnings,
    })
}

