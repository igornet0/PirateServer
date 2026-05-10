//! Post-check after certbot + nginx reload: `nginx -t` and optional local HTTPS probe through nginx.

use deploy_control::https_probe_localhost_resolve_with_retries;
use deploy_proto::deploy::{SslPostCheckDetail, SslPostCheckResult};

use super::config::SslProcessConfig;

/// Concrete SNI host: optional `SSL_POST_CHECK_HOST`, else primary (if not wildcard), else first non-wildcard domain.
pub fn resolve_ssl_probe_host(
    config: &SslProcessConfig,
    domains: &[String],
    primary: &str,
) -> Option<String> {
    let o = config.post_check_host.trim();
    if !o.is_empty() && !o.starts_with("*.") {
        return Some(o.to_string());
    }
    concrete_probe_host(domains, primary)
}

/// Primary first (typical certbot lineage), then first non-wildcard domain; `None` if only wildcards.
pub fn concrete_probe_host(domains: &[String], primary: &str) -> Option<String> {
    let p = primary.trim();
    if !p.is_empty() && !p.starts_with("*.") {
        return Some(p.to_string());
    }
    for d in domains {
        let t = d.trim();
        if t.is_empty() || t.starts_with("*.") {
            continue;
        }
        return Some(t.to_string());
    }
    None
}

fn add_detail(out: &mut SslPostCheckResult, step: &str, ok: bool, message: &str) {
    out.details.push(SslPostCheckDetail {
        step: step.to_string(),
        ok,
        message: message.to_string(),
    });
}

/// Run `sudo -n …/pirate-nginx-ops.sh validate` when `SSL_USE_SUDO` is enabled.
pub async fn nginx_validate_async(config: &SslProcessConfig, out: &mut SslPostCheckResult) {
    const SCRIPT: &str = "/usr/local/lib/pirate/pirate-nginx-ops.sh";
    if !config.use_sudo {
        out.nginx_test_ok = true;
        add_detail(
            out,
            "nginx_validate",
            true,
            "skipped: SSL_USE_SUDO disabled (cannot run sudo nginx validate)",
        );
        return;
    }
    let script = std::env::var("PIRATE_NGINX_OPS_SCRIPT").unwrap_or_else(|_| SCRIPT.to_string());
    let o = tokio::process::Command::new("sudo")
        .args(["-n", &script, "validate"])
        .output()
        .await;
    match o {
        Ok(o) if o.status.success() => {
            out.nginx_test_ok = true;
            add_detail(
                out,
                "nginx_validate",
                true,
                &String::from_utf8_lossy(&o.stdout).trim().to_string(),
            );
        }
        Ok(o) => {
            out.nginx_test_ok = false;
            let msg = [
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr),
            ]
            .concat();
            add_detail(out, "nginx_validate", false, &msg);
        }
        Err(e) => {
            out.nginx_test_ok = false;
            add_detail(out, "nginx_validate", false, &e.to_string());
        }
    }
}

/// Strict reload: return `Err` on non-zero exit when `strict_nginx_reload` is set.
pub async fn reload_nginx_result(
    config: &SslProcessConfig,
    out: &mut SslPostCheckResult,
) -> Result<(), String> {
    const SCRIPT: &str = "/usr/local/lib/pirate/pirate-nginx-ops.sh";
    if config.use_sudo {
        let script = std::env::var("PIRATE_NGINX_OPS_SCRIPT").unwrap_or_else(|_| SCRIPT.to_string());
        let r = tokio::process::Command::new("sudo")
            .args(["-n", &script, "reload"])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status.success() {
            let msg = [
                String::from_utf8_lossy(&r.stdout),
                String::from_utf8_lossy(&r.stderr),
            ]
            .concat();
            out.reload_ok = false;
            add_detail(out, "nginx_reload", false, &msg);
            let e = format!("nginx reload failed: {msg}");
            if config.strict_nginx_reload {
                return Err(e);
            }
            return Ok(());
        }
        out.reload_ok = true;
        add_detail(out, "nginx_reload", true, "systemctl reload nginx (via ops script)");
        return Ok(());
    }
    if let Some(cmd) = &config.reload_cmd {
        let r = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status.success() {
            let msg = String::from_utf8_lossy(&r.stderr).to_string();
            out.reload_ok = false;
            add_detail(out, "nginx_reload", false, &msg);
            let e = format!("SSL_RELOAD_CMD failed: {msg}");
            if config.strict_nginx_reload {
                return Err(e);
            }
            return Ok(());
        }
        out.reload_ok = true;
        add_detail(out, "nginx_reload", true, "SSL_RELOAD_CMD ok");
        return Ok(());
    }
    out.reload_ok = true;
    add_detail(
        out,
        "nginx_reload",
        true,
        "no reload: SSL_USE_SUDO=0 and no SSL_RELOAD_CMD",
    );
    Ok(())
}

/// HTTPS GET via curl locally through nginx; sets TLS/chain/upstream fields from probe outcome.
pub fn apply_https_probe(
    config: &SslProcessConfig,
    host: &str,
    out: &mut SslPostCheckResult,
) {
    if !config.post_check_enabled {
        return;
    }
    let pr = https_probe_localhost_resolve_with_retries(
        host,
        &config.post_check_path,
        config.post_check_port,
        &config.post_check_loopback,
        3,
        200,
    );
    out.http_status = pr.http_status as i32;
    out.probe_host = pr.probe_host.clone();
    out.curl_exit = pr.curl_exit;
    if pr.classified == "skipped_wildcard_or_empty" {
        out.tls_handshake_ok = false;
        out.chain_ok = false;
        out.upstream_health_ok = true;
        out.hostname_match_ok = false;
        add_detail(
            out,
            "https_probe",
            true,
            "HTTPS probe skipped (no concrete SNI host)",
        );
        return;
    }
    if pr.classified == "curl_unavailable" {
        out.classified_error = "curl_unavailable".to_string();
        out.tls_handshake_ok = false;
        out.chain_ok = false;
        out.upstream_health_ok = true;
        out.hostname_match_ok = true;
        add_detail(out, "https_probe", true, &pr.detail);
        return;
    }
    if matches!(
        pr.classified.as_str(),
        "tls_name_mismatch" | "tls_peer_cert" | "tls_cert_problem"
    ) {
        out.tls_handshake_ok = false;
        out.chain_ok = false;
        out.hostname_match_ok = false;
        out.upstream_health_ok = false;
    } else {
        out.tls_handshake_ok = pr.tls_effective;
        out.chain_ok = pr.tls_effective;
        out.hostname_match_ok = pr.tls_effective;
        out.upstream_health_ok = pr.ok;
    }
    if !pr.classified.is_empty() {
        out.classified_error = pr.classified.clone();
    }
    add_detail(
        out,
        "https_probe",
        pr.ok,
        &format!(
            "host={} http_status={} {} {}",
            pr.probe_host, pr.http_status, pr.classified, pr.detail
        ),
    );
}

/// Full pipeline after a cert is on disk: validate (optional) → reload → HTTPS probe to `host`.
pub async fn run_post_check_after_cert(
    config: &SslProcessConfig,
    host: &str,
) -> SslPostCheckResult {
    let mut out = SslPostCheckResult {
        nginx_test_ok: true,
        reload_ok: true,
        tls_handshake_ok: true,
        hostname_match_ok: true,
        chain_ok: true,
        upstream_health_ok: true,
        rollback_performed: false,
        summary: String::new(),
        details: vec![],
        http_status: 0,
        classified_error: String::new(),
        probe_host: String::new(),
        curl_exit: 0,
    };
    if !config.post_check_enabled {
        out.summary = "post_check disabled (SSL_POST_CHECK_ENABLED=0)".to_string();
        return out;
    }
    if host.trim().is_empty() {
        out.classified_error = "no_concrete_host".to_string();
        out.summary =
            "degraded: no concrete domain for HTTPS probe (wildcard-only SAN list)".to_string();
        out.upstream_health_ok = true; // n/a: cannot probe
        out.tls_handshake_ok = false;
        out.chain_ok = false;
        return out;
    }
    nginx_validate_async(config, &mut out).await;
    if let Err(e) = reload_nginx_result(config, &mut out).await {
        out.summary = format!("degraded: {e}");
        return out;
    }
    apply_https_probe(config, host, &mut out);
    if out.classified_error == "curl_unavailable" {
        out.summary =
            "ok (limited): install `curl` on the host to validate HTTPS end-to-end".to_string();
    } else if out.classified_error == "tls_name_mismatch" {
        out.summary = "degraded: HTTPS probe reports TLS hostname/SAN mismatch (check nginx server_name vs certificate)".to_string();
    } else if out.nginx_test_ok
        && out.reload_ok
        && out.upstream_health_ok
        && out.classified_error != "connect_failed"
        && out.classified_error != "upstream_5xx"
    {
        out.summary = "ok: nginx validate, reload, HTTPS smoke check (local resolve)".to_string();
    } else {
        out.summary = "degraded: see post_check details (e.g. 502, upstream, or reload)".to_string();
    }
    out
}
