//! Local project registry (`pirate-projects.json`) — thin wrappers for the desktop UI.

use deploy_core::pirate_project::PirateManifest;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredProject {
    pub name: String,
    pub path: String,
    /// `[project].version` in local `pirate.toml`.
    pub local_version: String,
    /// gRPC id used for GetStatus (`[project].deploy_project_id` or `default`).
    pub deploy_project_id: String,
    /// `[project].version` from server active release (empty if offline / error).
    pub server_project_version: String,
    pub connected: bool,
    /// Local manifest version differs from server (needs redeploy).
    pub needs_deploy: bool,
}

/// Sorted list for stable UI; when connected, compares local vs server manifest version.
pub fn list_registered_projects() -> Result<Vec<RegisteredProject>, String> {
    let map = deploy_client::list_projects()?;
    let has_endpoint = crate::connection::load_endpoint().is_some();

    let mut v: Vec<RegisteredProject> = Vec::new();
    for (name, path) in map {
        let pb = PathBuf::from(&path);
        let manifest_path = pb.join("pirate.toml");
        let manifest = PirateManifest::read_file(&manifest_path)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        let local_version = manifest.project.version.trim().to_string();
        let deploy_project_id = manifest.project.deploy_target_project_id();

        let (connected, server_project_version) = if has_endpoint {
            match crate::connection::verify_grpc_status_for_project(&deploy_project_id) {
                Ok(r) => (true, r.project_version.trim().to_string()),
                Err(_) => (false, String::new()),
            }
        } else {
            (false, String::new())
        };

        let needs_deploy = connected
            && !local_version.is_empty()
            && local_version != server_project_version;

        v.push(RegisteredProject {
            name,
            path,
            local_version,
            deploy_project_id,
            server_project_version,
            connected,
            needs_deploy,
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
