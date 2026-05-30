//! Persisted nginx vhost apply state under `{project_root}/.pirate/nginx_vhost_state.json`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// SHA-256 hex (lowercase) of a string.
pub fn sha256_str(s: &str) -> String {
    sha256_bytes(s.as_bytes())
}

/// SHA-256 hex (lowercase) of bytes.
pub fn sha256_bytes(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    hex::encode(h.finalize())
}

/// SHA-256 hex of file contents; empty string if unreadable.
pub fn sha256_file(path: &Path) -> Option<String> {
    let raw = std::fs::read(path).ok()?;
    Some(sha256_bytes(&raw))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct NginxVhostState {
    /// Hash of raw template (before placeholder substitution).
    pub template_sha256: String,
    /// Hash of processed vhost written to release / sites-available.
    pub applied_content_sha256: String,
    pub applied_version: String,
    pub applied_site_path: String,
    pub applied_at_ms: i64,
}

pub fn nginx_vhost_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".pirate").join("nginx_vhost_state.json")
}

pub fn read_nginx_vhost_state(project_root: &Path) -> Option<NginxVhostState> {
    let raw = std::fs::read_to_string(nginx_vhost_state_path(project_root)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_nginx_vhost_state(
    project_root: &Path,
    state: &NginxVhostState,
) -> std::io::Result<()> {
    let dir = project_root.join(".pirate");
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("nginx_vhost_state.json.tmp");
    let json = serde_json::to_string_pretty(state).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, nginx_vhost_state_path(project_root))?;
    Ok(())
}

/// Whether processed vhost content differs from persisted state or live sites-available file.
pub fn nginx_vhost_content_changed(
    _content: &str,
    content_sha256: &str,
    state: Option<&NginxVhostState>,
    site_path: &Path,
) -> bool {
    if state
        .map(|s| s.applied_content_sha256.as_str() != content_sha256)
        .unwrap_or(true)
    {
        return true;
    }
    match std::fs::read_to_string(site_path) {
        Ok(existing) if sha256_str(&existing) == content_sha256 => false,
        Ok(_) => true,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_str_is_stable() {
        let h = sha256_str("hello");
        assert_eq!(h.len(), 64);
        assert_eq!(h, sha256_str("hello"));
        assert_ne!(h, sha256_str("world"));
    }

    #[test]
    fn write_and_read_state_roundtrip() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("deploy-core-nginx-state-{pid}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let state = NginxVhostState {
            template_sha256: "abc".into(),
            applied_content_sha256: "def".into(),
            applied_version: "1.0.0".into(),
            applied_site_path: "/etc/nginx/sites-available/pirate-project-p-x".into(),
            applied_at_ms: 42,
        };
        write_nginx_vhost_state(&root, &state).unwrap();
        let loaded = read_nginx_vhost_state(&root).expect("read");
        assert_eq!(loaded, state);
        let _ = std::fs::remove_dir_all(&root);
    }
}
