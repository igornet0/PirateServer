//! Local project registry (`pirate-projects.json`) — thin wrappers for the desktop UI.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredProject {
    pub name: String,
    pub path: String,
    /// Cached `[project].version` from registry (refresh via re-add folder).
    pub local_version: String,
    /// gRPC id used for GetStatus (`[project].deploy_project_id` or `default`).
    pub deploy_project_id: String,
    /// `[project].version` from server active release (empty if offline / error).
    pub server_project_version: String,
    pub connected: bool,
    /// Local manifest version differs from server (needs redeploy).
    pub needs_deploy: bool,
    /// Unix millis when this folder was last deployed from the desktop (`None` → never recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_deploy_at_ms: Option<i64>,
    /// Version reported by server after last desktop deploy (`None` → unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_deployed_version: Option<String>,
}

/// Sorted list from registry cache + optional gRPC version compare.
pub fn list_registered_projects() -> Result<Vec<RegisteredProject>, String> {
    let rows = deploy_client::list_project_registry_entries()?;
    let has_endpoint = crate::connection::load_endpoint().is_some();

    let mut v: Vec<RegisteredProject> = Vec::new();
    for e in rows {
        let deploy_project_id = e.deploy_project_id.trim().to_string();
        let local_version = e.local_version.trim().to_string();

        let (connected, server_project_version) = if has_endpoint {
            match crate::connection::verify_grpc_status_for_project(&deploy_project_id) {
                Ok(r) => (true, r.project_version.trim().to_string()),
                Err(_) => (false, String::new()),
            }
        } else {
            (false, String::new())
        };

        let needs_deploy =
            connected && !local_version.is_empty() && local_version != server_project_version;

        v.push(RegisteredProject {
            name: e.name,
            path: e.path,
            local_version,
            deploy_project_id,
            server_project_version,
            connected,
            needs_deploy,
            last_deploy_at_ms: e.last_deploy_at_ms,
            last_deployed_version: e.last_deployed_version.clone(),
        });
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}

pub fn register_project_from_directory(path: String) -> Result<String, String> {
    deploy_client::register_from_pirate_toml_dir(PathBuf::from(path).as_path())
}

pub fn remove_registered_project(name: String) -> Result<bool, String> {
    deploy_client::remove_project_registry(&name)
}

/// Update registry after deploy (matches by canonical project folder path).
pub fn record_deploy_for_directory(
    directory: impl AsRef<std::path::Path>,
    deployed_version: &str,
    manifest_version_deployed: Option<&str>,
) -> Result<(), String> {
    deploy_client::record_deploy_for_project_root(
        directory.as_ref(),
        deployed_version,
        manifest_version_deployed,
    )
}
