use deploy_core::pirate_project::{resolve_project_env_join, PirateManifest};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalEnvView {
    /// Relative path from pirate.toml [project].env_path (e.g. ".env.prod").
    pub env_path: String,
    pub content: String,
    pub exists: bool,
}

pub fn read_project_local_env(directory: PathBuf) -> Result<LocalEnvView, String> {
    let manifest_path = directory.join("pirate.toml");
    let manifest = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let env_path_rel = manifest.project.effective_env_path_rel();
    let env_path = resolve_project_env_join(&directory, &manifest)?;
    let exists = env_path.exists();
    let content = if exists {
        std::fs::read_to_string(&env_path).map_err(|e| format!("read {env_path_rel}: {e}"))?
    } else {
        String::new()
    };
    Ok(LocalEnvView {
        env_path: env_path_rel,
        content,
        exists,
    })
}

pub fn write_project_local_env(directory: PathBuf, content: String) -> Result<(), String> {
    let manifest_path = directory.join("pirate.toml");
    let manifest = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let env_path_rel = manifest.project.effective_env_path_rel();
    let env_path = resolve_project_env_join(&directory, &manifest)?;
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir for {env_path_rel}: {e}"))?;
    }
    std::fs::write(&env_path, content).map_err(|e| format!("write {env_path_rel}: {e}"))
}
