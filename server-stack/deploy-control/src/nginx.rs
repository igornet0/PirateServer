use crate::types::{
    NginxConfigView, NginxEnsureView, NginxPutResponseView, NginxStatusView,
};
use crate::ControlError;
use deploy_core::nginx_snippet;
use deploy_core::pirate_project::PirateManifest;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn output_text(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{stdout}{stderr}")
}

pub fn nginx_route_conflicts(manifest: &PirateManifest) -> Vec<String> {
    let mut out = Vec::<String>::new();
    let mut seen = std::collections::BTreeSet::<String>::new();
    for k in manifest.proxy.routes.keys() {
        if !seen.insert(k.to_string()) {
            out.push(format!("duplicate nginx route `{k}`"));
        }
        if !k.starts_with('/') {
            out.push(format!("nginx route `{k}` must start with `/`"));
        }
    }
    out
}

pub fn generate_nginx_server_config(manifest: &PirateManifest) -> Result<String, ControlError> {
    let server_name = if manifest.network.access.domain.trim().is_empty() {
        "_".to_string()
    } else {
        manifest.network.access.domain.trim().to_string()
    };
    let routes = deploy_core::nginx_snippet::resolve_nginx_upstream_routes(manifest);
    if routes.is_empty() {
        return Err(ControlError::NginxOp(
            "no routes for nginx config generation".to_string(),
        ));
    }
    let mut blocks = String::new();
    for (path, target) in routes {
        blocks.push_str(&format!(
            r#"
    location {} {{
        proxy_pass http://{};
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }}
"#,
            path, target
        ));
    }
    let listen_port = if manifest.proxy.port > 0 && manifest.proxy.port < 1024 {
        manifest.proxy.port
    } else {
        80
    };
    Ok(format!(
        r#"# Pirate: project vhost (control-api apply)
server {{
    listen {listen_port};
    listen [::]:{listen_port};
    server_name {server_name};
{blocks}}}
"#,
        listen_port = listen_port,
        server_name = server_name,
        blocks = blocks
    ))
}

/// Path for a per-project nginx site under `sites-available`.
pub fn project_nginx_site_path(sites_available_dir: &Path, project_id: &str) -> PathBuf {
    let id = deploy_core::normalize_project_id(project_id);
    sites_available_dir.join(format!("pirate-project-{id}"))
}

/// Write project vhost, enable site symlink, validate and reload nginx.
pub fn apply_project_nginx_vhost(
    project_id: &str,
    manifest_toml: &str,
    sites_available_dir: &Path,
    apply_site_script: &Path,
    ops_script: &Path,
) -> Result<crate::types::ProjectNginxApplyView, ControlError> {
    use crate::nginx_universal::apply_nginx_universal_action;
    use crate::types::{NginxActionBody, ProjectNginxApplyView};

    let manifest = PirateManifest::parse(manifest_toml)
        .map_err(|e| ControlError::NginxOp(format!("invalid manifest: {e}")))?;
    let expected = deploy_core::normalize_project_id(project_id);
    let actual = manifest.project.deploy_target_project_id();
    if actual != expected {
        return Err(ControlError::NginxOp(format!(
            "manifest project id `{actual}` does not match path `{expected}`"
        )));
    }
    if let Err(e) = manifest.validate_network_proxy() {
        return Err(ControlError::NginxOp(e));
    }

    let mut warnings = nginx_route_conflicts(&manifest);
    if !nginx_snippet::nginx_edge_intended(&manifest) {
        warnings.push(
            "proxy is not configured as nginx edge; vhost will still be written".to_string(),
        );
    }

    let content = generate_nginx_server_config(&manifest)?;
    let site_path = project_nginx_site_path(sites_available_dir, project_id);

    let put = apply_nginx_site_via_sudo(&site_path, &content, apply_site_script)?;
    if !put.ok {
        return Ok(ProjectNginxApplyView {
            ok: false,
            path: site_path.display().to_string(),
            enabled: false,
            message: put.message,
            test_output: put.test_output,
            warnings,
        });
    }

    let enable = apply_nginx_universal_action(
        apply_site_script,
        ops_script,
        &NginxActionBody {
            action: "enable_site".into(),
            available_path: Some(site_path.display().to_string()),
            path: None,
            enabled_path: None,
            server_name: None,
            ssl_enabled: None,
            ssl_cert_path: None,
            ssl_key_path: None,
            issue_certificate_if_missing: None,
            post_check_host: None,
            post_check_enabled: None,
            post_check_path: None,
            post_check_port: None,
            post_check_loopback: None,
            acme_email: None,
            acme_staging: None,
            acme_dry_run: None,
        },
    )?;

    let enabled = enable.ok;
    let message = if enabled {
        format!(
            "project nginx site applied at {}; {}",
            site_path.display(),
            enable.message
        )
    } else {
        format!(
            "site file written at {} but enable failed: {}",
            site_path.display(),
            enable.message
        )
    };

    Ok(ProjectNginxApplyView {
        ok: put.ok && enabled,
        path: site_path.display().to_string(),
        enabled,
        message,
        test_output: put
            .test_output
            .or(enable.detail),
        warnings,
    })
}

/// Read nginx config file for API response.
pub async fn read_nginx_config(path: &Path) -> Result<NginxConfigView, std::io::Error> {
    let content = tokio::fs::read_to_string(path).await?;
    Ok(NginxConfigView {
        path: path.display().to_string(),
        content,
        enabled: true,
    })
}

const MAX_NGINX_INVENTORY_READ_BYTES: u64 = 256 * 1024;

/// Inventory editor path: absolute, under `/etc/nginx/`, no `..`.
pub fn parse_nginx_inventory_path(raw: &str) -> Result<PathBuf, ControlError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(ControlError::NginxOp("empty path".into()));
    }
    const PREFIX: &str = "/etc/nginx/";
    if !t.starts_with(PREFIX) || t.len() <= PREFIX.len() {
        return Err(ControlError::NginxOp(
            "path must be a file under /etc/nginx/".into(),
        ));
    }
    let p = Path::new(t);
    if !p.is_absolute() {
        return Err(ControlError::NginxOp("path must be absolute".into()));
    }
    for c in p.components() {
        if c == std::path::Component::ParentDir {
            return Err(ControlError::NginxOp("path must not contain '..'".into()));
        }
    }
    Ok(p.to_path_buf())
}

fn assert_not_sites_enabled_for_put(path: &Path) -> Result<(), ControlError> {
    if path.to_string_lossy().contains("/sites-enabled/") {
        return Err(ControlError::NginxOp(
            "refusing to write under sites-enabled; edit sites-available and enable the site instead"
                .into(),
        ));
    }
    Ok(())
}

/// Read a single inventory file (size-capped) for `GET /api/v1/nginx/file`.
pub async fn read_nginx_inventory_file(path: &Path) -> Result<NginxConfigView, std::io::Error> {
    let meta = tokio::fs::metadata(path).await?;
    if meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is a directory",
        ));
    }
    if meta.len() > MAX_NGINX_INVENTORY_READ_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file exceeds {} bytes", MAX_NGINX_INVENTORY_READ_BYTES),
        ));
    }
    read_nginx_config(path).await
}

/// Write inventory file: `sites-available/*` via apply-site script; others via `apply-config`.
pub async fn apply_nginx_inventory_file_put(
    path: &Path,
    content: &str,
    test_full_config: bool,
    ops_script: &Path,
    apply_site_script: &Path,
) -> Result<NginxPutOutcome, std::io::Error> {
    assert_not_sites_enabled_for_put(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    let lossy = path.to_string_lossy();
    if lossy.contains("/sites-available/") {
        let r = apply_nginx_site_via_sudo(path, content, apply_site_script).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })?;
        return Ok(NginxPutOutcome { response: r });
    }
    apply_nginx_put(path, content, test_full_config, ops_script).await
}

pub struct NginxPutOutcome {
    pub response: NginxPutResponseView,
}

/// Write config, `nginx -t`, optionally `nginx -s reload`. On test failure, returns `revert_to` content.
const MAX_NGINX_PUT_BYTES: usize = 256 * 1024;

/// Write main (or other) nginx config via `pirate-nginx-ops.sh apply-config` (root `nginx -t` + reload).
pub async fn apply_nginx_put(
    path: &Path,
    content: &str,
    test_full_config: bool,
    ops_script: &Path,
) -> Result<NginxPutOutcome, std::io::Error> {
    if content.len() > MAX_NGINX_PUT_BYTES {
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: format!("content exceeds {MAX_NGINX_PUT_BYTES} bytes"),
                test_output: None,
                reload_output: None,
            },
        });
    }
    if content.as_bytes().contains(&0) {
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: "content must not contain NUL bytes".into(),
                test_output: None,
                reload_output: None,
            },
        });
    }
    if !ops_script.is_file() {
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: format!(
                    "nginx ops script not found: {} (install pirate-nginx-ops.sh)",
                    ops_script.display()
                ),
                test_output: None,
                reload_output: None,
            },
        });
    }

    let path_buf = path.to_path_buf();
    let ops_buf = ops_script.to_path_buf();
    let content_owned = content.to_string();
    let test_full = test_full_config;

    let result = tokio::task::spawn_blocking(move || {
        let p = path_buf
            .to_str()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
        let ops = ops_buf
            .to_str()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid ops path"))?;
        let mut cmd = std::process::Command::new("sudo");
        cmd.args(["-n", ops, "apply-config", p]);
        if test_full {
            cmd.arg("full_main");
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no stdin"))?;
            use std::io::Write;
            stdin.write_all(content_owned.as_bytes())?;
        }
        child.wait_with_output()
    })
    .await??;

    let merged = output_text(&result);

    if !result.status.success() {
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: "nginx apply-config failed (see test_output; file reverted by helper if it existed)"
                    .into(),
                test_output: Some(merged),
                reload_output: None,
            },
        });
    }

    tracing::info!(path = %path.display(), "nginx config updated via ops helper");
    Ok(NginxPutOutcome {
        response: NginxPutResponseView {
            ok: true,
            message: "nginx config applied via privileged helper (test + reload)".into(),
            test_output: Some(merged),
            reload_output: None,
        },
    })
}

/// Снимок состояния nginx (для вкладки «nginx» в desktop).
pub fn collect_nginx_status(
    site_path: &Path,
    ensure_script: &Path,
    apply_script: &Path,
    ops_script: &Path,
) -> NginxStatusView {
    let installed = Command::new("sh")
        .args(["-c", "command -v nginx"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let version = if installed {
        Command::new("nginx")
            .arg("-v")
            .output()
            .ok()
            .and_then(|o| {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let line = if !stderr.is_empty() {
                    stderr
                } else {
                    stdout
                };
                line.lines()
                    .next()
                    .map(|l| l.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
    } else {
        None
    };

    let systemd_active = Command::new("systemctl")
        .args(["is-active", "nginx"])
        .output()
        .ok()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if o.status.success() {
                if s.is_empty() {
                    "active".to_string()
                } else {
                    s
                }
            } else if s == "inactive" || s == "failed" {
                s
            } else {
                "inactive".to_string()
            }
        });

    let site_file_exists = site_path.is_file();
    let site_enabled = Path::new("/etc/nginx/sites-enabled/pirate")
        .symlink_metadata()
        .is_ok();

    NginxStatusView {
        installed,
        version,
        systemd_active,
        site_config_path: site_path.display().to_string(),
        site_file_exists,
        site_enabled,
        ensure_script_present: ensure_script.is_file(),
        apply_site_script_present: apply_script.is_file(),
        ops_script_present: ops_script.is_file(),
    }
}

pub fn read_nginx_site_file(path: &Path) -> NginxConfigView {
    if path.is_file() {
        let content = fs::read_to_string(path).unwrap_or_default();
        NginxConfigView {
            path: path.display().to_string(),
            content,
            enabled: true,
        }
    } else {
        NginxConfigView {
            path: path.display().to_string(),
            content: String::new(),
            enabled: false,
        }
    }
}

const MAX_NGINX_SITE_BYTES: usize = 256 * 1024;

/// Запись vhost через `sudo pirate-nginx-apply-site.sh` (nginx -t + systemctl reload).
pub fn apply_nginx_site_via_sudo(
    path: &Path,
    content: &str,
    helper: &Path,
) -> Result<NginxPutResponseView, ControlError> {
    if content.len() > MAX_NGINX_SITE_BYTES {
        return Err(ControlError::NginxOp(format!(
            "content exceeds {} bytes",
            MAX_NGINX_SITE_BYTES
        )));
    }
    if content.as_bytes().contains(&0) {
        return Err(ControlError::NginxOp(
            "content must not contain NUL bytes".into(),
        ));
    }

    if !helper.is_file() {
        return Err(ControlError::NginxOp(format!(
            "helper script not found: {}",
            helper.display()
        )));
    }

    let target = path
        .to_str()
        .ok_or_else(|| ControlError::NginxOp("invalid site path".into()))?;

    let mut child = Command::new("sudo")
        .args([
            "-n",
            helper
                .to_str()
                .ok_or_else(|| ControlError::NginxOp("invalid helper path".into()))?,
            target,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ControlError::NginxOp(format!("sudo: {e}")))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ControlError::NginxOp("sudo: stdin not available".into()))?;
    stdin
        .write_all(content.as_bytes())
        .map_err(|e| ControlError::NginxOp(format!("stdin: {e}")))?;
    drop(stdin);

    let out = child
        .wait_with_output()
        .map_err(|e| ControlError::NginxOp(format!("sudo wait: {e}")))?;

    let merged = output_text(&out);
    if !out.status.success() {
        return Ok(NginxPutResponseView {
            ok: false,
            message: format!("nginx apply failed: {}", merged.trim()),
            test_output: Some(merged.trim().to_string()),
            reload_output: None,
        });
    }

    Ok(NginxPutResponseView {
        ok: true,
        message: merged.trim().to_string(),
        test_output: Some(merged.trim().to_string()),
        reload_output: None,
    })
}

/// Установка/удаление nginx и сайта Pirate (`api_only`, `with_ui`, `remove`).
pub fn ensure_nginx_via_sudo(mode: &str, helper: &Path) -> Result<NginxEnsureView, ControlError> {
    if mode != "api_only" && mode != "with_ui" && mode != "remove" {
        return Err(ControlError::NginxOp(
            "mode must be api_only, with_ui or remove".into(),
        ));
    }
    if !helper.is_file() {
        return Err(ControlError::NginxOp(format!(
            "ensure script not found: {}",
            helper.display()
        )));
    }

    let out = Command::new("sudo")
        .args([
            "-n",
            helper
                .to_str()
                .ok_or_else(|| ControlError::NginxOp("invalid helper path".into()))?,
            mode,
        ])
        .output()
        .map_err(|e| ControlError::NginxOp(format!("sudo: {e}")))?;

    let merged = output_text(&out);
    if !out.status.success() {
        return Ok(NginxEnsureView {
            ok: false,
            message: "ensure nginx failed".into(),
            output: Some(merged),
            env_update: None,
        });
    }

    Ok(NginxEnsureView {
        ok: true,
        message: merged.trim().to_string(),
        output: Some(merged),
        env_update: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::pirate_project::PirateManifest;

    fn manifest_for_nginx_gen() -> PirateManifest {
        PirateManifest::parse(
            r#"
[project]
name = "x"
version = "1"
deploy_project_id = "p-testnginx01"

[services.web]
type = "http"
port = 3000
source = "t"
confidence = 1.0

[proxy]
type = "nginx"
port = 3000
enabled = true

[network]
mode = "wan"

[network.access]
public = true
domain = "app.example.com"
"#,
        )
        .expect("parse")
    }

    #[test]
    fn generate_nginx_server_config_listens_on_80_and_proxies_loopback() {
        let cfg = generate_nginx_server_config(&manifest_for_nginx_gen()).expect("config");
        assert!(cfg.contains("listen 80;"), "{cfg}");
        assert!(cfg.contains("server_name app.example.com"), "{cfg}");
        assert!(cfg.contains("proxy_pass http://127.0.0.1:3000"), "{cfg}");
        assert!(cfg.contains("# Pirate: project vhost"), "{cfg}");
    }

    #[test]
    fn project_nginx_site_path_uses_project_id() {
        let p = project_nginx_site_path(Path::new("/etc/nginx/sites-available"), "p-abc123");
        assert_eq!(
            p,
            PathBuf::from("/etc/nginx/sites-available/pirate-project-p-abc123")
        );
    }
}
