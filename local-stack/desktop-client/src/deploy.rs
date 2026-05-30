//! gRPC deploy / rollback from the desktop (reuse `deploy-client` packing + upload).

use crate::connection::{
    load_control_api_base, load_endpoint, load_project_id, load_signing_key_for_endpoint,
};
use crate::control_api::{
    control_api_bearer_token, control_api_deploy_artifact_multipart, format_bytes_with_mib,
};
use deploy_auth::attach_auth_metadata;
use deploy_client::{
    detect_services, generate_proxy_config, pack_directory_for_deploy,
    resolve_proxy_route_upstream, upload_packed_tar_gz_grpc, validate_version_label,
    DeployValidationReport, ServiceDetectionReport,
};

pub use deploy_client::DeployProgressEvent;
use deploy_core::pirate_project::PirateManifest;

use crate::host_services_compat::{
    summarize_host_services_for_manifest, HostServicesCompatSummary,
};
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

    let packed = pack_directory_for_deploy(&dir, &version, &project, chunk_size, &on_event).await?;

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
                    nginx_apply: None,
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
                nginx_apply: nginx_apply_from_proto(summary.response.nginx_apply),
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
                        nginx_apply: None,
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

fn nginx_apply_from_proto(
    na: Option<deploy_proto::deploy::DeployNginxApply>,
) -> Option<DeployNginxApplyOutcome> {
    na.map(|n| DeployNginxApplyOutcome {
        ok: n.ok,
        path: n.path,
        message: n.message,
        action: n.action,
        template_changed: n.template_changed,
        content_sha256: n.content_sha256,
        test_output: if n.test_output.is_empty() {
            None
        } else {
            Some(n.test_output)
        },
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployNginxApplyOutcome {
    pub ok: bool,
    pub path: String,
    pub message: String,
    pub action: String,
    pub template_changed: bool,
    pub content_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_output: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nginx_apply: Option<DeployNginxApplyOutcome>,
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
    /// When true, nginx gets WebSocket upgrade headers for this path.
    #[serde(default)]
    pub websocket: bool,
}

/// Network wizard fields persisted into `pirate.toml` before server-side nginx apply.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProjectNetworkManifestInput {
    /// `local` | `lan` | `public`
    pub access_mode: String,
    /// Primary hostname (first in `domains` when that list is set).
    #[serde(default)]
    pub domain: String,
    /// All public hostnames; first becomes `network.access.domain`, rest `network.access.domains`.
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub routes: Vec<NetworkAccessRouteOverride>,
    #[serde(default)]
    pub https_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadProjectNetworkManifestView {
    pub access_mode: String,
    pub domain: String,
    pub domains: Vec<String>,
    pub routes: Vec<NetworkAccessRouteOverride>,
    pub https_enabled: bool,
    pub static_nginx_front: bool,
}

fn collect_public_domains(domain: &str, domains: &[String]) -> Vec<String> {
    let mut out = Vec::<String>::new();
    for d in std::iter::once(domain).chain(domains.iter().map(String::as_str)) {
        let t = d.trim();
        if t.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn access_mode_from_manifest_mode(mode: &str, _public: bool) -> String {
    match deploy_core::pirate_project::normalize_network_mode(mode) {
        "wan" => "public".to_string(),
        "lan" => "lan".to_string(),
        _ => "local".to_string(),
    }
}

fn is_static_proxy_type(proxy_type: &str) -> bool {
    matches!(
        proxy_type.trim().to_ascii_lowercase().as_str(),
        "nginx-front" | "nginx-static"
    )
}

/// Write `[network]` / `[proxy]` from the desktop network wizard into `pirate.toml`.
pub fn save_project_network_manifest(
    project_dir: PathBuf,
    input: SaveProjectNetworkManifestInput,
) -> Result<(), String> {
    use std::collections::BTreeMap;

    let manifest_path = project_dir.join("pirate.toml");
    let mut m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    let mode = match input.access_mode.trim().to_ascii_lowercase().as_str() {
        "public" => "wan",
        "lan" => "lan",
        "local" | "private" => "private",
        other => {
            return Err(format!(
                "invalid access_mode `{other}` (expected local|lan|public)"
            ));
        }
    };
    m.network.mode = mode.to_string();
    m.network.access.public = mode == "wan";
    if mode == "wan" {
        let hosts = collect_public_domains(&input.domain, &input.domains);
        m.network.access.domain = hosts.first().cloned().unwrap_or_default();
        m.network.access.domains = hosts.into_iter().skip(1).collect();
    } else {
        m.network.access.domain = String::new();
        m.network.access.domains.clear();
    }
    m.network.tls.https = input.https_enabled && mode == "wan";

    let static_front = is_static_proxy_type(&m.proxy.r#type) || deploy_core::pirate_project::is_static_nginx_edge(&m);
    if static_front {
        if mode == "wan" {
            let hosts = collect_public_domains(&input.domain, &input.domains);
            if m.proxy.domain.trim().is_empty() {
                if let Some(d) = hosts.first() {
                    m.proxy.domain = d.clone();
                }
            }
        }
    } else {
        m.proxy.r#type = "nginx".to_string();
        m.proxy.enabled = mode != "private";
        m.proxy.backend = String::new();

        let mut routes = BTreeMap::<String, String>::new();
        let mut websocket_paths = Vec::<String>::new();
        for r in &input.routes {
            let path = r.path.trim();
            if path.is_empty() {
                continue;
            }
            let upstream = resolve_proxy_route_upstream(r.target.trim())?;
            routes.insert(path.to_string(), upstream);
            if r.websocket {
                websocket_paths.push(path.to_string());
            }
        }
        m.proxy.routes = routes;
        m.proxy.websocket_paths = websocket_paths;
    }

    let s = m
        .to_toml_string()
        .map_err(|e| format!("serialize pirate.toml: {e}"))?;
    std::fs::write(&manifest_path, s).map_err(|e| format!("write {}: {e}", manifest_path.display()))
}

/// Read network wizard fields from `pirate.toml` (for desktop UI prefill).
pub fn load_project_network_manifest(
    project_dir: PathBuf,
) -> Result<LoadProjectNetworkManifestView, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let static_nginx_front = deploy_core::pirate_project::is_static_nginx_edge(&m);
    let mut access_mode = access_mode_from_manifest_mode(&m.network.mode, m.network.access.public);
    let mut hosts = deploy_core::pirate_project::network_access_server_names(&m.network.access);
    if static_nginx_front && hosts.is_empty() {
        let d = m.proxy.domain.trim();
        if !d.is_empty() {
            hosts.push(d.to_string());
            if deploy_core::pirate_project::normalize_network_mode(&m.network.mode) == "wan" {
                access_mode = "public".to_string();
            }
        }
    }
    let domain = hosts.first().cloned().unwrap_or_default();
    let domains = if hosts.len() > 1 {
        hosts[1..].to_vec()
    } else {
        Vec::new()
    };
    let ws_set: std::collections::BTreeSet<String> = m
        .proxy
        .websocket_paths
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let routes = m
        .proxy
        .routes
        .iter()
        .map(|(path, target)| NetworkAccessRouteOverride {
            path: path.clone(),
            target: target.clone(),
            websocket: ws_set.contains(path),
        })
        .collect();
    Ok(LoadProjectNetworkManifestView {
        access_mode,
        domain,
        domains,
        routes,
        https_enabled: m.network.tls.https,
        static_nginx_front,
    })
}

/// UI-only overrides so nginx preview matches the network wizard before `pirate.toml` is saved.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeNetworkAccessOverrides {
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub domains: Option<Vec<String>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_nginx_template_sha256: Option<String>,
}

/// Shared process runtime (see `tokio_runtime`).
pub fn runtime() -> Result<std::sync::Arc<tokio::runtime::Runtime>, String> {
    crate::tokio_runtime::shared_runtime()
}

/// Pack `dir` and upload to the saved gRPC endpoint (same as `client deploy`).
pub fn run_deploy(
    dir: PathBuf,
    version: String,
    chunk_size: usize,
) -> Result<DeployOutcome, String> {
    validate_version_label(&version).map_err(|e| e.to_string())?;
    let rt = runtime()?;
    rt.block_on(async {
        deploy_with_grpc_and_http_fallback(dir, version, chunk_size, |_| {}).await
    })
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
    let endpoint =
        load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
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
    let endpoint =
        load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
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
                m_preview.proxy.websocket_paths.clear();
                for r in routes {
                    let path = r.path.trim();
                    if path.is_empty() {
                        continue;
                    }
                    m_preview
                        .proxy
                        .routes
                        .insert(path.to_string(), r.target.clone());
                    if r.websocket {
                        m_preview.proxy.websocket_paths.push(path.to_string());
                    }
                }
            }
        }
        if let Some(ref domains) = o.domains {
            let hosts = collect_public_domains("", domains);
            if !hosts.is_empty() {
                m_preview.network.access.domain = hosts[0].clone();
                m_preview.network.access.domains = hosts.into_iter().skip(1).collect();
            }
        } else if let Some(ref d) = o.domain {
            let t = d.trim();
            if !t.is_empty() {
                m_preview.network.access.domain = t.to_string();
                m_preview.network.access.domains.clear();
            }
        }
    }

    let server_name: String = {
        let names =
            deploy_core::pirate_project::network_access_server_names(&m_preview.network.access);
        if names.is_empty() {
            "_".to_string()
        } else {
            names.join(" ")
        }
    };

    let nginx_preview = generate_proxy_config(&m_preview, Some(&server_name)).ok();
    let host_services = summarize_host_services_for_manifest(&m);
    let local_nginx_template_sha256 = if m.proxy.has_custom_nginx_template() {
        deploy_core::pirate_project::resolve_nginx_conf_join(&project_dir, &m)
            .ok()
            .and_then(|p| deploy_core::nginx_vhost_state::sha256_file(&p))
            .filter(|h| !h.is_empty())
    } else {
        None
    };
    Ok(NetworkAccessAnalysis {
        project_id: m.project.deploy_target_project_id(),
        detection,
        nginx_preview,
        host_services,
        local_nginx_template_sha256,
    })
}

pub fn validate_network_access_remote(
    project_dir: PathBuf,
) -> Result<DeployValidationReport, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest_toml = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let project_id = m.project.deploy_target_project_id();
    let client_nginx_template_sha256 = if m.proxy.has_custom_nginx_template() {
        deploy_core::pirate_project::resolve_nginx_conf_join(&project_dir, &m)
            .ok()
            .and_then(|p| deploy_core::nginx_vhost_state::sha256_file(&p))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let endpoint =
        load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let rt = runtime()?;
    let report = rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let mut req = Request::new(ValidateDeployRequest {
            project_id: project_id.clone(),
            manifest_toml,
            client_nginx_template_sha256,
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
        nginx_state: report.nginx_state.map(|s| deploy_client::NginxDeployStateView {
            server_template_sha256: s.server_template_sha256,
            applied_content_sha256: s.applied_content_sha256,
            applied_site_path: s.applied_site_path,
            last_applied_at_ms: s.last_applied_at_ms,
            needs_update: s.needs_update,
        }),
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
