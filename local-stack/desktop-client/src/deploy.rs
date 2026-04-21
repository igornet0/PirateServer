//! gRPC deploy / rollback from the desktop (reuse `deploy-client` packing + upload).

use crate::connection::{
    load_control_api_base, load_endpoint, load_project_id, load_signing_key_for_endpoint,
};
use crate::control_api::{
    control_api_bearer_token, control_api_deploy_artifact_multipart, format_bytes_with_mib,
};
use deploy_auth::attach_auth_metadata;
use deploy_client::{
    detect_services, generate_proxy_config, pack_directory_for_deploy, upload_packed_tar_gz_grpc,
    validate_version_label, DeployValidationReport, ServiceDetectionReport,
};

pub use deploy_client::DeployProgressEvent;
use deploy_core::pirate_project::PirateManifest;

use crate::host_services_compat::{summarize_host_services_for_manifest, HostServicesCompatSummary};
use deploy_proto::deploy::{RemoveProjectRequest, RollbackRequest, ValidateDeployRequest};
use deploy_proto::DeployServiceClient;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tonic::Request;

fn enrich_deploy_limit_error(e: String) -> String {
    if e.contains("artifact exceeds limit") || e.contains("exceeds HTTP deploy limit") {
        format!(
            "{e} — на сервере увеличьте DEPLOY_MAX_UPLOAD_BYTES (и перезапустите deploy-server и control-api с тем же значением) или войдите в control-api в клиенте для автоматической выгрузки через HTTP POST …/deploy-artifact (multipart)."
        )
    } else {
        e
    }
}

fn is_artifact_limit_error(msg: &str) -> bool {
    msg.contains("artifact exceeds limit") || msg.contains("exceeds HTTP deploy limit")
}

/// Returns `(control_api_base, bearer_jwt)` when the desktop can call multipart deploy.
fn http_multipart_deploy_ready() -> Option<(String, String)> {
    let base = load_control_api_base()?;
    let base = base.trim();
    if base.is_empty() {
        return None;
    }
    let token = control_api_bearer_token().ok()?;
    Some((base.to_string(), token))
}

async fn deploy_with_grpc_and_http_fallback<F>(
    dir: PathBuf,
    version: String,
    chunk_size: usize,
    on_event: F,
) -> Result<DeployOutcome, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    let endpoint =
        load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    validate_version_label(&version).map_err(|e| e.to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let project = load_project_id();

    let max_upload = crate::connection::verify_grpc_status_for_project_async(&project)
        .await
        .ok()
        .and_then(|r| r.max_upload_bytes);

    let on_event = Arc::new(Mutex::new(on_event));

    let packed = pack_directory_for_deploy(
        &dir,
        &version,
        &project,
        chunk_size,
        &on_event,
    )
    .await?;

    if let Some(limit) = max_upload {
        if packed.artifact_bytes > limit {
            if let Some((base, token)) = http_multipart_deploy_ready() {
                let r = control_api_deploy_artifact_multipart(
                    &base,
                    &project,
                    &version,
                    &packed.path,
                    packed.manifest_toml.as_deref(),
                    &token,
                    packed.artifact_bytes,
                    &on_event,
                )
                .await;
                let _ = tokio::fs::remove_file(&packed.path).await;
                let r = r.map_err(enrich_deploy_limit_error)?;
                return Ok(DeployOutcome {
                    status: r.status,
                    deployed_version: r.deployed_version,
                    artifact_bytes: packed.artifact_bytes,
                    chunk_count: packed.chunk_count,
                    upload_channel: Some(
                        if r.used_chunked_session {
                            "http_chunked"
                        } else {
                            "http_multipart"
                        }
                        .into(),
                    ),
                });
            }
            let _ = tokio::fs::remove_file(&packed.path).await;
            return Err(enrich_deploy_limit_error(format!(
                "artifact exceeds limit of {}; packed {}; sign in to control-api for automatic HTTP multipart upload or increase DEPLOY_MAX_UPLOAD_BYTES on the server",
                format_bytes_with_mib(limit),
                format_bytes_with_mib(packed.artifact_bytes)
            )));
        }
    }

    let grpc_res = upload_packed_tar_gz_grpc(
        &endpoint,
        &packed,
        &version,
        &project,
        chunk_size,
        sk.as_ref(),
        &on_event,
    )
    .await;

    match grpc_res {
        Ok(summary) => {
            let _ = tokio::fs::remove_file(&packed.path).await;
            Ok(DeployOutcome {
                status: summary.response.status,
                deployed_version: summary.response.deployed_version,
                artifact_bytes: summary.artifact_bytes,
                chunk_count: summary.chunk_count,
                upload_channel: Some("grpc".into()),
            })
        }
        Err(e) if is_artifact_limit_error(&e) => {
            if let Some((base, token)) = http_multipart_deploy_ready() {
                let r = control_api_deploy_artifact_multipart(
                    &base,
                    &project,
                    &version,
                    &packed.path,
                    packed.manifest_toml.as_deref(),
                    &token,
                    packed.artifact_bytes,
                    &on_event,
                )
                .await;
                let _ = tokio::fs::remove_file(&packed.path).await;
                match r {
                    Ok(out) => Ok(DeployOutcome {
                        status: out.status,
                        deployed_version: out.deployed_version,
                        artifact_bytes: packed.artifact_bytes,
                        chunk_count: packed.chunk_count,
                        upload_channel: Some(
                            if out.used_chunked_session {
                                "http_chunked"
                            } else {
                                "http_multipart"
                            }
                            .into(),
                        ),
                    }),
                    Err(e2) => Err(enrich_deploy_limit_error(format!(
                        "gRPC: {e}; HTTP multipart retry: {e2}"
                    ))),
                }
            } else {
                let _ = tokio::fs::remove_file(&packed.path).await;
                Err(enrich_deploy_limit_error(e))
            }
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&packed.path).await;
            Err(enrich_deploy_limit_error(e))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployOutcome {
    pub status: String,
    pub deployed_version: String,
    pub artifact_bytes: u64,
    pub chunk_count: usize,
    /// `grpc` or `http_multipart` when known; omitted in JSON if absent (older clients).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_channel: Option<String>,
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
    /// True if a managed child process was stopped (kill + wait).
    pub had_managed_process: bool,
    /// Ports probed after stop (127.0.0.1 bind); empty if none.
    pub verified_listen_ports: Vec<u32>,
    /// Present only on success; empty on success (failure uses gRPC error).
    pub ports_still_busy: Vec<u32>,
    pub removed_deploy_events_rows: u64,
    pub removed_project_snapshots_rows: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeployCheck {
    pub project_id: String,
    pub uploaded: bool,
    pub current_version: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAccessRouteOverride {
    pub path: String,
    pub target: String,
}

/// UI-only overrides so nginx preview matches the network wizard before `pirate.toml` is saved.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeNetworkAccessOverrides {
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub routes: Option<Vec<NetworkAccessRouteOverride>>,
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

pub fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// Pack `dir` and upload to the saved gRPC endpoint (same as `client deploy`).
pub fn run_deploy(dir: PathBuf, version: String, chunk_size: usize) -> Result<DeployOutcome, String> {
    validate_version_label(&version).map_err(|e| e.to_string())?;
    let rt = runtime()?;
    rt.block_on(async { deploy_with_grpc_and_http_fallback(dir, version, chunk_size, |_| {}).await })
}

/// Async deploy with per-stage progress (Tauri UI uses `deploy-progress` events).
pub async fn run_deploy_with_progress_events<F>(
    dir: PathBuf,
    version: String,
    chunk_size: usize,
    on_event: F,
) -> Result<DeployOutcome, String>
where
    F: FnMut(DeployProgressEvent) + Send + 'static,
{
    deploy_with_grpc_and_http_fallback(dir, version, chunk_size, on_event).await
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
        had_managed_process: inner.had_managed_process,
        verified_listen_ports: inner.verified_listen_ports,
        ports_still_busy: inner.ports_still_busy,
        removed_deploy_events_rows: inner.removed_deploy_events_rows,
        removed_project_snapshots_rows: inner.removed_project_snapshots_rows,
    })
}

pub fn analyze_network_access(
    project_dir: PathBuf,
    overrides: Option<AnalyzeNetworkAccessOverrides>,
) -> Result<NetworkAccessAnalysis, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let mut m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let detection = detect_services(&project_dir, Some(&m));
    deploy_client::apply_detected_services_to_manifest(&mut m, &detection);

    let mut m_preview = m.clone();
    if let Some(ref o) = overrides {
        if let Some(ref routes) = o.routes {
            if !routes.is_empty() {
                m_preview.proxy.routes.clear();
                for r in routes {
                    m_preview.proxy.routes.insert(r.path.clone(), r.target.clone());
                }
            }
        }
    }

    let server_name: String = overrides
        .as_ref()
        .and_then(|o| o.domain.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let d = m_preview.network.access.domain.trim();
            if d.is_empty() {
                None
            } else {
                Some(d.to_string())
            }
        })
        .unwrap_or_else(|| "_".to_string());

    let nginx_preview = generate_proxy_config(&m_preview, &server_name).ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_artifact_limit_error_detects_server_message() {
        assert!(is_artifact_limit_error(
            "artifact exceeds limit of 10485760 bytes"
        ));
        assert!(is_artifact_limit_error(
            "packed artifact … exceeds HTTP deploy limit …"
        ));
        assert!(!is_artifact_limit_error("connection refused"));
    }

    #[test]
    fn enrich_deploy_limit_error_appends_hint() {
        let e = enrich_deploy_limit_error("artifact exceeds limit of 10 bytes".into());
        assert!(e.contains("DEPLOY_MAX_UPLOAD_BYTES"));
        assert!(e.contains("control-api"));
    }

    #[test]
    fn enrich_deploy_limit_error_passes_through_other_errors() {
        let e = enrich_deploy_limit_error("network error".into());
        assert_eq!(e, "network error");
    }
}

