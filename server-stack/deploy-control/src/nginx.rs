use crate::types::{
    NginxConfigView, NginxEnsureView, NginxPutResponseView, NginxStatusView,
};
use crate::ControlError;
use deploy_core::pirate_project::PirateManifest;
use std::fs;
use std::io::Write;
use std::path::Path;
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
    Ok(format!(
        r#"server {{
    listen {};
    server_name {};
{}
}}
"#,
        manifest.proxy.port.max(1),
        server_name,
        blocks
    ))
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

pub struct NginxPutOutcome {
    pub response: NginxPutResponseView,
}

/// Write config, `nginx -t`, optionally `nginx -s reload`. On test failure, returns `revert_to` content.
pub async fn apply_nginx_put(
    path: &Path,
    content: &str,
    test_full_config: bool,
) -> Result<NginxPutOutcome, std::io::Error> {
    let previous = tokio::fs::read_to_string(path).await.unwrap_or_default();
    tokio::fs::write(path, content).await?;

    let path_owned = path.to_path_buf();
    let test_full = test_full_config;
    let nginx_test_output_result = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("nginx");
        cmd.arg("-t");
        if test_full {
            cmd.arg("-c").arg(&path_owned);
        }
        cmd.output()
    })
    .await??;

    let test_out = output_text(&nginx_test_output_result);

    if !nginx_test_output_result.status.success() {
        let _ = tokio::fs::write(path, &previous).await;
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: "nginx -t failed; file reverted".into(),
                test_output: Some(test_out),
                reload_output: None,
            },
        });
    }

    let reload_path = path.to_path_buf();
    let reload_full = test_full_config;
    let reload_res = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("nginx");
        if reload_full {
            cmd.arg("-c").arg(&reload_path);
        }
        cmd.arg("-s").arg("reload").output()
    })
    .await??;

    let reload_out = output_text(&reload_res);

    if !reload_res.status.success() {
        tracing::warn!(%reload_out, "nginx reload failed");
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: "nginx -t ok but nginx -s reload failed (config left on disk)".into(),
                test_output: Some(test_out),
                reload_output: Some(reload_out),
            },
        });
    }

    tracing::info!(path = %path.display(), "nginx config updated and reloaded");
    Ok(NginxPutOutcome {
        response: NginxPutResponseView {
            ok: true,
            message: "nginx config written, nginx -t passed, reload sent".into(),
            test_output: Some(test_out),
            reload_output: Some(reload_out),
        },
    })
}

/// Снимок состояния nginx (для вкладки «nginx» в desktop).
pub fn collect_nginx_status(
    site_path: &Path,
    ensure_script: &Path,
    apply_script: &Path,
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
