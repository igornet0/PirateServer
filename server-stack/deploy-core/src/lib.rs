//! Shared deployment root layout, version validation, and [`AppState`] for the deploy service.

use std::path::{Path, PathBuf};

/// Max length for a version label (directory name under `releases/`).
pub const MAX_VERSION_LEN: usize = 128;

/// Max length for a non-default project id segment under `projects/<id>/`.
pub const MAX_PROJECT_ID_LEN: usize = 64;

/// Normalize `project_id`: empty or `default` → deploy to legacy `--root` layout.
pub fn normalize_project_id(project_id: &str) -> String {
    let s = project_id.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("default") {
        "default".to_string()
    } else {
        s.to_string()
    }
}

/// Validate a project id (single path segment; same charset as version).
pub fn validate_project_id(project_id: &str) -> Result<(), &'static str> {
    let n = normalize_project_id(project_id);
    if n == "default" {
        return Ok(());
    }
    if n.len() > MAX_PROJECT_ID_LEN {
        return Err("project_id too long");
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err("project_id may only contain [a-zA-Z0-9._-]");
    }
    if n.contains("..") || n.contains('/') || n.contains('\\') {
        return Err("invalid project_id");
    }
    Ok(())
}

/// Deploy root for a project: legacy `default` uses `base`; others use `base/projects/<id>/`.
pub fn project_deploy_root(base: &Path, project_id: &str) -> PathBuf {
    let n = normalize_project_id(project_id);
    if n == "default" {
        base.to_path_buf()
    } else {
        base.join("projects").join(n)
    }
}

/// Runtime state shared between gRPC handlers (and mirrored to DB for UI).
pub struct AppState {
    pub child: Option<tokio::process::Child>,
    pub current_version: String,
    /// `running` | `stopped` | `error`
    pub state: String,
    pub last_error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            child: None,
            current_version: String::new(),
            state: "stopped".to_string(),
            last_error: None,
        }
    }
}

/// Validate version string for use as a directory name (same rules as server).
pub fn validate_version(version: &str) -> Result<(), &'static str> {
    if version.is_empty() {
        return Err("version must not be empty");
    }
    if version.len() > MAX_VERSION_LEN {
        return Err("version too long");
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err("version may only contain [a-zA-Z0-9._-]");
    }
    if version.contains("..") || version.contains('/') || version.contains('\\') {
        return Err("invalid version string");
    }
    Ok(())
}

pub fn releases_dir(root: &Path) -> PathBuf {
    root.join("releases")
}

pub fn release_dir_for_version(root: &Path, version: &str) -> PathBuf {
    releases_dir(root).join(version)
}

pub fn read_current_version_from_symlink(root: &Path) -> Option<String> {
    let link = root.join("current");
    let target = std::fs::read_link(&link).ok()?;
    target.file_name()?.to_str().map(|s| s.to_string())
}

/// Sorted list of subdirectory names under `releases/`.
pub fn list_release_versions(root: &Path) -> std::io::Result<Vec<String>> {
    let dir = releases_dir(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            if let Some(name) = e.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn refresh_process_state(st: &mut AppState) {
    if let Some(ref mut c) = st.child {
        match c.try_wait() {
            Ok(Some(status)) => {
                st.child = None;
                if status.success() {
                    st.state = "stopped".to_string();
                } else {
                    st.state = "error".to_string();
                    st.last_error = Some(format!("process exited: {status}"));
                }
            }
            Ok(None) => {
                st.state = "running".to_string();
            }
            Err(e) => {
                st.state = "error".to_string();
                st.last_error = Some(e.to_string());
                st.child = None;
            }
        }
    } else if st.current_version.is_empty() {
        st.state = "stopped".to_string();
    }
}
