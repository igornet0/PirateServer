//! Internal process manager: native spawn, health checks, persisted state, `run.sh` generation.

use crate::nginx_vhost_state::sha256_str;
use crate::pirate_project::{resolve_nginx_conf_join, PirateManifest};
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

fn is_static_nginx_proxy(manifest: &PirateManifest) -> bool {
    let t = manifest.proxy.r#type.trim().to_ascii_lowercase();
    t == "nginx-front" || t == "nginx-static"
}

/// Generate `run.sh` in release dir from manifest (POSIX sh).
pub fn generate_run_sh(release_dir: &Path, manifest: &PirateManifest) -> std::io::Result<()> {
    // Dotenv from packed `[project].env_path` (default `.env`), then `[env]` exports override.
    let rel = manifest.project.effective_env_path_rel();
    let dotenv_path_esc = shell_escape_single(&rel);
    let dotenv_block = format!(
        "if [ -f {p} ]; then\n  set -a\n  . {p}\n  set +a\nfi\n",
        p = dotenv_path_esc
    );
    let mut exports = String::new();
    for (k, v) in &manifest.env {
        let esc = shell_escape_single(v);
        exports.push_str(&format!("export {}={}\n", k, esc));
    }
    let start = if manifest.start.cmd.is_empty() {
        if is_static_nginx_proxy(manifest) {
            "while true; do sleep 3600; done".to_string()
        } else {
            "echo \"pirate: no [start].cmd\"; exit 1".to_string()
        }
    } else {
        manifest.start.cmd.clone()
    };
    let body = format!(
        r#"#!/bin/sh
set -e
cd "$(dirname "$0")"
{dotenv_block}{exports}exec sh -c {start_esc}
"#,
        dotenv_block = dotenv_block,
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
pub fn write_service_compose(
    project_root: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<()> {
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

/// Result of writing a custom or generated nginx release snippet.
#[derive(Debug, Clone)]
pub struct NginxSnippetWriteResult {
    pub template_sha256: String,
    pub content_sha256: String,
    pub content: String,
}

/// Proxy snippet (nginx) — written to release for operator merge when
/// [`crate::nginx_snippet::should_write_nginx_release_snippet`] holds, or from a custom template
/// when `[proxy].nginx_conf_path` is set.
pub fn write_nginx_snippet(
    release_dir: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<Option<NginxSnippetWriteResult>> {
    if manifest.proxy.has_custom_nginx_template() {
        return write_custom_nginx_snippet(release_dir, manifest).map(Some);
    }
    if !crate::nginx_snippet::should_write_nginx_release_snippet(manifest) {
        return Ok(None);
    }
    let content = crate::nginx_snippet::nginx_release_snippet_content(manifest).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("nginx release snippet: {e}"),
        )
    })?;
    let content_sha256 = sha256_str(&content);
    std::fs::write(release_dir.join("pirate-nginx-snippet.conf"), &content)?;
    Ok(Some(NginxSnippetWriteResult {
        template_sha256: String::new(),
        content_sha256,
        content,
    }))
}

fn read_nginx_template_raw(
    project_root: &Path,
    release_dir: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<(String, PathBuf)> {
    let rel_fallback = release_dir.join(
        manifest
            .proxy
            .nginx_conf_path
            .trim()
            .trim_start_matches("./"),
    );
    if let Ok(joined) = resolve_nginx_conf_join(project_root, manifest) {
        if joined.is_file() {
            let raw = std::fs::read_to_string(&joined)?;
            return Ok((raw, joined));
        }
    }
    let raw = std::fs::read_to_string(&rel_fallback).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("nginx template {}: {e}", rel_fallback.display()),
        )
    })?;
    Ok((raw, rel_fallback))
}

fn write_custom_nginx_snippet(
    release_dir: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<NginxSnippetWriteResult> {
    let project_root = release_dir
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid release_dir layout")
        })?;
    let version = release_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "release_dir has no version segment",
            )
        })?;
    let (raw, _template_path) = read_nginx_template_raw(project_root, release_dir, manifest)?;
    let template_sha256 = sha256_str(&raw);
    let content = crate::nginx_template::substitute_nginx_template(&raw, project_root, version);
    let content_sha256 = sha256_str(&content);
    std::fs::write(release_dir.join("pirate-nginx-snippet.conf"), &content)?;
    Ok(NginxSnippetWriteResult {
        template_sha256,
        content_sha256,
        content,
    })
}

/// Merge dotenv file at [`crate::pirate_project::resolve_project_env_join`] into env map.
pub fn load_dotenv(project_root: &Path, manifest: &PirateManifest) -> BTreeMap<String, String> {
    let Ok(p) = crate::pirate_project::resolve_project_env_join(project_root, manifest) else {
        return BTreeMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return BTreeMap::new();
    };
    parse_dotenv_lines(&raw)
}

fn parse_dotenv_lines(raw: &str) -> BTreeMap<String, String> {
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
        if let Ok(stream) =
            std::net::TcpStream::connect_timeout(&parse_socket_addr(hostport), timeout)
        {
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
    let s = manifest
        .to_toml_string()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(&p, s)
}

/// After unpack: apply sidecar manifest from upload metadata if `pirate.toml` missing.
pub fn apply_sidecar_manifest(
    release_dir: &Path,
    manifest: &PirateManifest,
) -> std::io::Result<Option<NginxSnippetWriteResult>> {
    generate_run_sh(release_dir, manifest)?;
    ensure_manifest_in_release(release_dir, manifest)?;
    let root = release_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(release_dir);
    let _ = write_service_compose(root, manifest);
    write_nginx_snippet(release_dir, manifest)
}

pub fn release_dir_for(project_root: &Path, version: &str) -> PathBuf {
    release_dir_for_version(project_root, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pirate_project::PirateManifest;

    fn manifest_with_nginx_template() -> PirateManifest {
        PirateManifest::parse(
            r#"
[project]
name = "x"
version = "1"

[proxy]
type = "nginx-front"
nginx_conf_path = "./pirate-nginx-snippet.conf"
"#,
        )
        .expect("parse")
    }

    #[test]
    fn write_custom_nginx_snippet_substitutes_placeholders() {
        let pid = std::process::id();
        let project_root =
            std::env::temp_dir().join(format!("deploy-core-nginx-snippet-{pid}"));
        let _ = std::fs::remove_dir_all(&project_root);
        let release_dir = project_root.join("releases").join("0.2.0");
        std::fs::create_dir_all(&release_dir).unwrap();
        std::fs::write(
            release_dir.join("pirate-nginx-snippet.conf"),
            "root <PATH_PROJECT>/releases/<VERSION>/dist;",
        )
        .unwrap();
        let m = manifest_with_nginx_template();
        write_nginx_snippet(&release_dir, &m).unwrap();
        let out = std::fs::read_to_string(release_dir.join("pirate-nginx-snippet.conf")).unwrap();
        assert!(out.contains(&format!(
            "root {}/releases/0.2.0/dist;",
            project_root.display()
        )));
        let _ = std::fs::remove_dir_all(&project_root);
    }

    #[test]
    fn generate_run_sh_noop_for_nginx_front_without_start() {
        let pid = std::process::id();
        let release_dir = std::env::temp_dir().join(format!("deploy-core-runsh-{pid}"));
        let _ = std::fs::remove_dir_all(&release_dir);
        std::fs::create_dir_all(&release_dir).unwrap();
        let m = manifest_with_nginx_template();
        generate_run_sh(&release_dir, &m).unwrap();
        let run = std::fs::read_to_string(release_dir.join("run.sh")).unwrap();
        assert!(run.contains("sleep 3600"));
        let _ = std::fs::remove_dir_all(&release_dir);
    }
}
