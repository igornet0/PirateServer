//! Shared deployment root layout, version validation, and [`AppState`] for the deploy service.

pub mod display_stream;
pub mod pirate_project;
pub mod process_manager;
pub mod sandbox;

/// Cargo package version of this crate (linked into `pirate` / deploy clients).
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Native install (and bundles that mimic it) write bundle metadata here.
pub const PIRATE_VAR_LIB: &str = "/var/lib/pirate";

/// Snapshot written by `install.sh`: GUI probe + `display_stream_consent` at install time.
pub const HOST_GUI_INSTALL_JSON: &str = "host-gui-install.json";

/// Parse `gui_detected` from `host-gui-install.json` body.
pub fn host_gui_detected_from_install_json(raw: &str) -> Option<bool> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()?
        .get("gui_detected")
        .and_then(|v| v.as_bool())
}

/// Systemd `EnvironmentFile` for deploy-server / control-api (see `install.sh`).
pub const PIRATE_DEPLOY_ENV_PATH: &str = "/etc/pirate-deploy.env";

/// True when env file defines non-empty JWT + UI admin credentials (dashboard auth enabled).
pub fn pirate_deploy_env_dashboard_enabled(contents: &str) -> bool {
    let mut jwt = false;
    let mut user = false;
    let mut pass = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("CONTROL_API_JWT_SECRET=") {
            jwt = !rest.trim().is_empty();
        } else if let Some(rest) = line.strip_prefix("CONTROL_UI_ADMIN_USERNAME=") {
            user = !rest.trim().is_empty();
        } else if let Some(rest) = line.strip_prefix("CONTROL_UI_ADMIN_PASSWORD=") {
            pass = !rest.trim().is_empty();
        }
    }
    jwt && user && pass
}

/// `Some(true)` if symlink exists, `Some(false)` if missing, `None` if we could not stat (e.g. permission).
pub fn host_nginx_pirate_site_enabled() -> Option<bool> {
    let p = Path::new("/etc/nginx/sites-enabled/pirate");
    match std::fs::symlink_metadata(p) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// Read dashboard flag from [`PIRATE_DEPLOY_ENV_PATH`] and nginx site presence.
pub fn read_host_stack_ui_flags() -> (bool, Option<bool>) {
    let dash = match std::fs::read_to_string(Path::new(PIRATE_DEPLOY_ENV_PATH)) {
        Ok(s) => pirate_deploy_env_dashboard_enabled(&s),
        Err(_) => false,
    };
    (dash, host_nginx_pirate_site_enabled())
}

/// Contents of `server-stack-version` when present and non-empty after trim.
pub fn read_server_stack_bundle_version_from_var_lib() -> Option<String> {
    let path = Path::new(PIRATE_VAR_LIB).join("server-stack-version");
    let s = std::fs::read_to_string(path).ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Shown as `GetStatus.current_version` when no app release is active (no `current` symlink).
/// The `stack@` prefix is not a valid [`validate_version`] label (`@` is disallowed), so it
/// cannot be mistaken for a directory under `releases/` or used as a rollback target.
pub fn idle_server_stack_status_label(deploy_server_pkg_version: &str) -> String {
    let tail = read_server_stack_bundle_version_from_var_lib()
        .unwrap_or_else(|| format!("binary-{}", deploy_server_pkg_version));
    format!("stack@{tail}")
}

/// Resolved application release for status, or [`idle_server_stack_status_label`] when none.
pub fn status_current_version_display(
    in_memory_or_symlink: &str,
    project_root: &Path,
    deploy_server_pkg_version: &str,
) -> String {
    let mut current = in_memory_or_symlink.to_string();
    if current.is_empty() {
        if let Some(v) = read_current_version_from_symlink(project_root) {
            current = v;
        }
    }
    if current.is_empty() {
        current = idle_server_stack_status_label(deploy_server_pkg_version);
    }
    current
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_stack_status_label_is_not_valid_app_version() {
        let s = idle_server_stack_status_label("9.9.9");
        assert!(s.starts_with("stack@"));
        assert!(validate_version(&s).is_err());
    }

    #[test]
    fn pirate_deploy_env_dashboard_reads_flags() {
        let s = r#"
DEPLOY_SQLITE_URL=sqlite:///x
CONTROL_API_JWT_SECRET=secret
CONTROL_UI_ADMIN_USERNAME=admin
CONTROL_UI_ADMIN_PASSWORD=pw
"#;
        assert!(pirate_deploy_env_dashboard_enabled(s));
        assert!(!pirate_deploy_env_dashboard_enabled("CONTROL_API_JWT_SECRET=\nCONTROL_UI_ADMIN_USERNAME=a\nCONTROL_UI_ADMIN_PASSWORD=b"));
    }

    #[test]
    fn host_gui_install_json_parses_gui_detected() {
        let raw = r#"{"gui_detected":true,"reasons":["x"],"display_stream_consent":1,"ts_unix":1}"#;
        assert_eq!(host_gui_detected_from_install_json(raw), Some(true));
        let raw2 = r#"{"gui_detected":false}"#;
        assert_eq!(host_gui_detected_from_install_json(raw2), Some(false));
    }

    #[test]
    #[cfg(unix)]
    fn status_display_uses_symlink_over_idle_label() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "deploy-core-status-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(releases_dir(&root).join("v9")).unwrap();
        symlink(Path::new("releases").join("v9"), root.join("current")).unwrap();
        let out = status_current_version_display("", &root, "0.0.1");
        assert_eq!(out, "v9");
        let _ = std::fs::remove_dir_all(&root);
    }
}
