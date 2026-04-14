//! Project init + scan (`pirate init-project`, `pirate scan-project`).

use crate::project_registry;
use deploy_core::pirate_project::{detect_runtime, guess_port, PirateManifest};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanReport {
    pub project_root: String,
    pub runtime: String,
    pub suggested_port: u16,
    pub has_dockerfile: bool,
    pub markers: Vec<String>,
    pub updated_pirate_toml: bool,
}

/// Initialize `pirate.toml` in `project_root` (default `.`).
pub fn init_project(project_root: &Path, name_override: Option<&str>) -> Result<PathBuf, String> {
    let root = project_root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", project_root.display()))?;
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let rt = detect_runtime(&root);
    let name = name_override
        .map(|s| s.to_string())
        .or_else(|| {
            root.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "app".to_string());

    let mut manifest = PirateManifest::default_for_project(&name, rt);
    let port = guess_port(&root, rt);
    manifest.proxy.port = port;
    manifest.health.port = port;

    let path = root.join("pirate.toml");
    if path.exists() {
        return Err(format!(
            "{} already exists; remove it or use scan-project",
            path.display()
        ));
    }
    let body = manifest
        .to_toml_string()
        .map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    let m = PirateManifest::read_file(&path).map_err(|e| format!("read back: {e}"))?;
    project_registry::register(&m.project.name, &root)?;
    Ok(path)
}

/// Scan project markers, optionally merge port/runtime into existing `pirate.toml`.
pub fn scan_project(project_root: &Path, dry_run: bool) -> Result<ScanReport, String> {
    let root = project_root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", project_root.display()))?;
    let rt = detect_runtime(&root);
    let port = guess_port(&root, rt);
    let mut markers = Vec::new();
    for (cond, label) in [
        (root.join("package.json").is_file(), "package.json"),
        (root.join("requirements.txt").is_file(), "requirements.txt"),
        (root.join("pyproject.toml").is_file(), "pyproject.toml"),
        (root.join("go.mod").is_file(), "go.mod"),
        (root.join("Cargo.toml").is_file(), "Cargo.toml"),
        (root.join("composer.json").is_file(), "composer.json"),
        (root.join("pom.xml").is_file(), "pom.xml"),
        (root.join("build.gradle").is_file(), "build.gradle"),
        (root.join("Dockerfile").is_file(), "Dockerfile"),
    ] {
        if cond {
            markers.push(label.to_string());
        }
    }

    let path = root.join("pirate.toml");
    let mut updated = false;
    if path.is_file() && !dry_run {
        let mut m = PirateManifest::read_file(&path).map_err(|e| format!("parse pirate.toml: {e}"))?;
        if m.runtime.r#type != rt {
            m.runtime.r#type = rt.to_string();
            updated = true;
        }
        if m.proxy.port != port || m.health.port != port {
            m.proxy.port = port;
            m.health.port = port;
            updated = true;
        }
        if updated {
            let body = m.to_toml_string().map_err(|e| e.to_string())?;
            std::fs::write(&path, body).map_err(|e| e.to_string())?;
        }
        let _ = project_registry::register_from_pirate_toml_dir(&root);
    }

    Ok(ScanReport {
        project_root: root.display().to_string(),
        runtime: rt.to_string(),
        suggested_port: port,
        has_dockerfile: root.join("Dockerfile").is_file(),
        markers,
        updated_pirate_toml: updated,
    })
}
