//! gRPC `UploadServerStack` / `GetServerStackInfo` (OTA host bundle).

use crate::connection::{load_endpoint, load_signing_key_for_endpoint};
use crate::deploy::runtime;
use deploy_client::{
    fetch_server_stack_info, read_or_pack_bundle, upload_server_stack_artifact_with_progress,
    validate_version_label,
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStackOutcome {
    pub status: String,
    pub applied_version: String,
    pub deploy_server_pkg_version: Option<String>,
    pub control_api_pkg_version: Option<String>,
}

/// Pack directory or read `.tar.gz`, upload as server-stack OTA.
pub fn run_server_stack_update_with_progress<F>(
    path: PathBuf,
    version: String,
    chunk_size: usize,
    on_progress: F,
) -> Result<ServerStackOutcome, String>
where
    F: FnMut(u64, u64) + Send + 'static,
{
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    validate_version_label(&version).map_err(|e| e.to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let artifact = read_or_pack_bundle(&path).map_err(|e| e.to_string())?;
    let rt = runtime()?;
    let resp = rt.block_on(upload_server_stack_artifact_with_progress(
        &endpoint,
        &artifact,
        &version,
        chunk_size,
        sk.as_ref(),
        None,
        on_progress,
    ))
    .map_err(|s| {
        let m = s.message();
        if m.is_empty() {
            format!("{s:?}")
        } else {
            m.to_string()
        }
    })?;
    Ok(ServerStackOutcome {
        status: resp.status,
        applied_version: resp.applied_version,
        deploy_server_pkg_version: resp.deploy_server_pkg_version,
        control_api_pkg_version: resp.control_api_pkg_version,
    })
}

pub fn fetch_server_stack_info_json() -> Result<String, String> {
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let rt = runtime()?;
    let info = rt.block_on(fetch_server_stack_info(&endpoint, sk.as_ref())).map_err(|s| {
        let m = s.message();
        if m.is_empty() {
            format!("{s:?}")
        } else {
            m.to_string()
        }
    })?;
    Ok(serde_json::json!({
        "bundleVersion": info.bundle_version,
        "manifestJson": info.manifest_json,
        "deployServerBinaryVersion": info.deploy_server_binary_version,
        "hostDashboardEnabled": info.host_dashboard_enabled,
        "hostNginxPirateSite": info.host_nginx_pirate_site,
    })
    .to_string())
}
