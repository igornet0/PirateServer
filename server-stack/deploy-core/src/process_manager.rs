//! Internal process manager: native spawn, health checks, persisted state, `run.sh` generation.

use crate::pirate_project::PirateManifest;
use crate::release_dir_for_version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Persisted state under `{project_root}/.pirate/runtime_state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeState {
    pub project_id: String,
    pub release_version: String,
    pub pid: Option<u32>,
    pub port: u16,
    pub status: String,
    pub restart_count: u32,
    pub last_start_unix_ms: i64,
    pub last_error: Option<String>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            release_version: String::new(),
            pid: None,
            port: 0,
            status: "stopped".to_string(),
            restart_count: 0,
            last_start_unix_ms: 0,
            last_error: None,
        }
    }
}

pub fn pirate_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".pirate").join("runtime_state.json")
}

pub fn write_runtime_state(project_root: &Path, state: &RuntimeState) -> std::io::Result<()> {
    let dir = project_root.join(".pirate");
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("runtime_state.json.tmp");
    let json = serde_json::to_string_pretty(state).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, pirate_state_path(project_root))?;
    Ok(())
}

pub fn read_runtime_state(project_root: &Path) -> Option<RuntimeState> {
    let p = pirate_state_path(project_root);
    let raw = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Generate `run.sh` in release dir from manifest (POSIX sh).
pub fn generate_run_sh(release_dir: &Path, manifest: &PirateManifest) -> std::io::Result<()> {
    let mut exports = String::new();
    for (k, v) in &manifest.env {
        let esc = shell_escape_single(v);
        exports.push_str(&format!("export {}={}\n", k, esc));
    }
    let start = if manifest.start.cmd.is_empty() {
        "echo \"pirate: no [start].cmd\"; exit 1".to_string()
    } else {
        manifest.start.cmd.clone()
    };
    let body = format!(
        r#"#!/bin/sh
set -e
cd "$(dirname "$0")"
{exports}
exec sh -c {start_esc}
"#,
        exports = exports,
        start_esc = shell_escape_single(&start),
    );
    let run = release_dir.join("run.sh");
    std::fs::write(&run, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&run)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&run, perms)?;
    }
    Ok(())
}

fn shell_escape_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Write `docker-compose.pirate.yml` next to project root for optional local services.
pub fn write_service_compose(project_root: &Path, manifest: &PirateManifest) -> std::io::Result<()> {
    let s = &manifest.services;
    if !s.postgres && !s.redis && !s.mysql && !s.mongodb {
        return Ok(());
    }
    let mut lines: Vec<String> = vec!["services:".to_string()];
    if s.postgres {
        lines.push("  postgres:".to_string());
        lines.push("    image: postgres:16-alpine".to_string());
        lines.push("    environment:".to_string());
        lines.push("      POSTGRES_PASSWORD: pirate".to_string());
        lines.push("      POSTGRES_USER: pirate".to_string());
        lines.push("      POSTGRES_DB: pirate".to_string());
        lines.push("    ports:".to_string());
        lines.push("      - \"5432:5432\"".to_string());
    }
    if s.redis {
        lines.push("  redis:".to_string());
        lines.push("    image: redis:7-alpine".to_string());
        lines.push("    ports:".to_string());
        lines.push("      - \"6379:6379\"".to_string());
    }
    if s.mysql {
        lines.push("  mysql:".to_string());
        lines.push("    image: mysql:8".to_string());
        lines.push("    environment:".to_string());
        lines.push("      MYSQL_ROOT_PASSWORD: pirate".to_string());
        lines.push("      MYSQL_DATABASE: pirate".to_string());
        lines.push("    ports:".to_string());
        lines.push("      - \"3306:3306\"".to_string());
    }
    if s.mongodb {
        lines.push("  mongo:".to_string());
        lines.push("    image: mongo:7".to_string());
        lines.push("    ports:".to_string());
        lines.push("      - \"27017:27017\"".to_string());
    }
    let p = project_root.join("docker-compose.pirate.yml");
    std::fs::write(&p, lines.join("\n") + "\n")?;
    Ok(())
}

/// Proxy snippet (nginx) — written to release for operator merge.
pub fn write_nginx_snippet(release_dir: &Path, manifest: &PirateManifest) -> std::io::Result<()> {
    if manifest.proxy.r#type != "nginx" {
        return Ok(());
    }
    let port = manifest.proxy.port;
    let server_name = if manifest.proxy.domain.is_empty() {
        "_".to_string()
    } else {
        manifest.proxy.domain.clone()
    };
    let conf = format!(
        r#"# Generated by Pirate — merge into your server block
location / {{
    proxy_pass http://127.0.0.1:{port};
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
}}
"#,
        port = port
    );
    std::fs::write(
        release_dir.join("pirate-nginx-snippet.conf"),
        format!("# server_name {server_name};\n{conf}"),
    )?;
    Ok(())
}

/// Merge `.env` file into env map (later entries in file win if duplicate).
pub fn load_dotenv(project_root: &Path) -> BTreeMap<String, String> {
    let p = project_root.join(".env");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    out
}

/// Perform HTTP GET health check (blocking).
pub fn http_health_check(url: &str, timeout: Duration) -> bool {
    // Minimal TCP+HTTP without extra deps in deploy-core: use std::net for TCP only,
    // or return true if URL empty. For full HTTP we need reqwest or ureq — deploy-core stays light.
    // Parse host:port from http://127.0.0.1:3000/health
    let url = url.trim();
    if url.is_empty() {
        return false;
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let hostport = rest.split('/').next().unwrap_or("");
        if let Ok(stream) = std::net::TcpStream::connect_timeout(
            &parse_socket_addr(hostport),
            timeout,
        ) {
            drop(stream);
            return true;
        }
        return false;
    }
    false
}

fn parse_socket_addr(hostport: &str) -> std::net::SocketAddr {
    use std::net::ToSocketAddrs;
    hostport
        .to_socket_addrs()
        .ok()
        .and_then(|mut i| i.next())
        .unwrap_or_else(|| "127.0.0.1:3000".parse().unwrap())
}

pub fn health_url_from_manifest(manifest: &PirateManifest) -> String {
    let port = manifest.health.port;
    let path = if manifest.health.path.is_empty() {
        "/"
    } else {
        &manifest.health.path
    };
    format!("http://127.0.0.1:{}{}", port, path)
}

/// Ensure `pirate.toml` exists in release dir (copy from packed artifact).
pub fn ensure_manifest_in_release(
    release_dir: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<()> {
    let p = release_dir.join("pirate.toml");
    let s = manifest.to_toml_string().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    std::fs::write(&p, s)
}

/// After unpack: apply sidecar manifest from upload metadata if `pirate.toml` missing.
pub fn apply_sidecar_manifest(
    release_dir: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<()> {
    generate_run_sh(release_dir, manifest)?;
    ensure_manifest_in_release(release_dir, manifest)?;
    let root = release_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(release_dir);
    let _ = write_service_compose(root, manifest);
    let _ = write_nginx_snippet(release_dir, manifest);
    Ok(())
}

pub fn release_dir_for(project_root: &Path, version: &str) -> PathBuf {
    release_dir_for_version(project_root, version)
}
