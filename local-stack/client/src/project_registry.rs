//! Local registry: `[project].name` from `pirate.toml` → absolute project root path.

use deploy_core::pirate_project::PirateManifest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const REGISTRY_FILE: &str = "pirate-projects.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    /// Project name → absolute path string
    projects: BTreeMap<String, String>,
}

fn registry_path() -> Result<PathBuf, String> {
    crate::config::config_dir()
        .ok_or_else(|| "no config directory (set XDG_CONFIG_HOME or equivalent)".to_string())
        .map(|d| d.join(REGISTRY_FILE))
}

fn load_raw() -> Result<RegistryFile, String> {
    let p = registry_path()?;
    if !p.is_file() {
        return Ok(RegistryFile::default());
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", p.display()))
}

fn save_raw(r: &RegistryFile) -> Result<(), String> {
    let p = registry_path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

/// Register `name` → canonical `root` (overwrites if same name).
pub fn register(name: &str, root: &Path) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("project name must not be empty".to_string());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let mut r = load_raw()?;
    r.projects.insert(name.to_string(), root.display().to_string());
    save_raw(&r)
}

/// Read `pirate.toml` and register `[project].name` → directory.
pub fn register_from_pirate_toml_dir(root: &Path) -> Result<String, String> {
    let p = root.join("pirate.toml");
    let m = PirateManifest::read_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let n = m.project.name.trim();
    if n.is_empty() {
        return Err("[project].name is empty in pirate.toml".to_string());
    }
    register(n, root)?;
    Ok(n.to_string())
}

/// Resolve registered project name to root path.
pub fn resolve_path(name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("project name must not be empty".to_string());
    }
    let r = load_raw()?;
    let s = r
        .projects
        .get(name)
        .ok_or_else(|| {
            format!(
                "unknown project name '{name}': run `pirate projects add <path>` or `pirate init-project`"
            )
        })?;
    let pb = PathBuf::from(s);
    if !pb.is_dir() {
        return Err(format!(
            "registered path for '{name}' is missing: {}",
            pb.display()
        ));
    }
    Ok(pb)
}

pub fn list_projects() -> Result<BTreeMap<String, String>, String> {
    Ok(load_raw()?.projects)
}

pub fn remove(name: &str) -> Result<bool, String> {
    let name = name.trim();
    let mut r = load_raw()?;
    let ok = r.projects.remove(name).is_some();
    save_raw(&r)?;
    Ok(ok)
}
