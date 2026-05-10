//! Invoke Certbot and capture stdout/stderr (no private keys).

use super::config::{SslModeResolved, SslProcessConfig};

#[derive(Debug, Clone)]
pub struct CertbotRunResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

fn certbot_argv_base(config: &SslProcessConfig) -> (std::path::PathBuf, Vec<String>) {
    #[cfg(unix)]
    let need_sudo = {
        let uid = unsafe { libc::geteuid() };
        uid != 0 && config.use_sudo
    };
    #[cfg(not(unix))]
    let need_sudo = false;

    if need_sudo {
        (
            std::path::PathBuf::from("sudo"),
            vec!["-n".to_string(), config.certbot_bin.clone()],
        )
    } else {
        (std::path::PathBuf::from(&config.certbot_bin), vec![])
    }
}

/// `certonly` for a new certificate.
pub async fn certonly(
    config: &SslProcessConfig,
    email: &str,
    domains: &[String],
    mode: SslModeResolved,
    webroot: Option<&str>,
    dry_run: bool,
    staging: bool,
) -> Result<CertbotRunResult, String> {
    if domains.is_empty() {
        return Err("no domains".into());
    }
    let (prog, prefix) = certbot_argv_base(config);
    let mut args: Vec<String> = prefix;
    args.extend(
        [
            "certonly".to_string(),
            "--non-interactive".to_string(),
            "--agree-tos".to_string(),
            "-m".to_string(),
            email.to_string(),
        ]
        .into_iter(),
    );
    if dry_run {
        args.push("--dry-run".to_string());
    }
    if staging {
        args.push("--test-cert".to_string());
    }
    args.extend(config.certbot_extra_args.iter().cloned());
    match mode {
        SslModeResolved::Nginx => {
            args.push("--nginx".to_string());
        }
        SslModeResolved::Standalone => {
            args.push("--standalone".to_string());
        }
        SslModeResolved::Webroot => {
            let w = webroot
                .or(config.webroot_path.as_deref())
                .ok_or("webroot path required (set SSL_WEBROOT or pass webroot in request)")?;
            args.push("--webroot".to_string());
            args.push("-w".to_string());
            args.push(w.to_string());
        }
        SslModeResolved::Dns => {
            // Typical: --dns-cloudflare --dns-cloudflare-credentials /path/to/ini
            if let (Some(p), Some(cred)) = (
                config.dns_plugin.as_deref().filter(|s| !s.is_empty()),
                config.dns_credentials.as_deref().filter(|s| !s.is_empty()),
            ) {
                if p != "cloudflare" {
                    return Err(format!(
                        "SSL_CERTBOT_DNS_PLUGIN={p}: only 'cloudflare' is wired in v1; use generic SSL_MODE + SSL_CERTBOT_EXTRA_ARGS for other DNS plugins"
                    ));
                }
                args.push("--dns-cloudflare".to_string());
                args.push("--dns-cloudflare-credentials".to_string());
                args.push(cred.to_string());
            } else {
                return Err(
                    "SSL_MODE=dns requires SSL_CERTBOT_DNS_PLUGIN=cloudflare and SSL_CERTBOT_DNS_CREDENTIALS=/path/ini (or set SSL_MODE=nginx/standalone/webroot)"
                        .into(),
                );
            }
        }
    }
    for d in domains {
        args.push("-d".to_string());
        args.push(d.clone());
    }
    run_process(&prog, &args).await
}

/// `certbot renew` (optionally for one line).
pub async fn renew(
    config: &SslProcessConfig,
    cert_name: Option<&str>,
    dry_run: bool,
) -> Result<CertbotRunResult, String> {
    let (prog, mut args) = certbot_argv_base(config);
    args.push("renew".to_string());
    if dry_run {
        args.push("--dry-run".to_string());
    }
    args.extend(config.certbot_extra_args.iter().cloned());
    if let Some(n) = cert_name {
        args.push("--cert-name".to_string());
        args.push(n.to_string());
    }
    run_process(&prog, &args).await
}

/// Force renew one line (expired or urgent).
pub async fn force_renew(
    config: &SslProcessConfig,
    cert_name: &str,
    dry_run: bool,
) -> Result<CertbotRunResult, String> {
    let (prog, mut args) = certbot_argv_base(config);
    args.extend(
        [
            "renew".to_string(),
            "--cert-name".to_string(),
            cert_name.to_string(),
            "--force-renewal".to_string(),
        ]
        .into_iter(),
    );
    if dry_run {
        args.push("--dry-run".to_string());
    }
    args.extend(config.certbot_extra_args.iter().cloned());
    run_process(&prog, &args).await
}

async fn run_process(
    program: &std::path::Path,
    args: &[String],
) -> Result<CertbotRunResult, String> {
    let mut c = tokio::process::Command::new(program);
    c.args(args);
    c.stdout(std::process::Stdio::piped());
    c.stderr(std::process::Stdio::piped());
    let out = c
        .output()
        .await
        .map_err(|e| format!("failed to start {}: {e}", program.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Ok(CertbotRunResult {
        status: out.status.code().unwrap_or(-1),
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use crate::ssl::config::{SslModeResolved, SslProcessConfig};

    fn test_config() -> SslProcessConfig {
        SslProcessConfig {
            provider: "certbot".to_string(),
            email: None,
            mode: SslModeResolved::Nginx,
            webroot_path: None,
            check_interval_secs: 86400,
            expiry_threshold_days: 7,
            certbot_bin: "certbot".to_string(),
            certbot_extra_args: vec![],
            dns_plugin: None,
            dns_credentials: None,
            reload_cmd: None,
            webhook_url: None,
            use_sudo: false,
            post_check_enabled: false,
            post_check_path: "/".to_string(),
            post_check_port: 443,
            post_check_loopback: "127.0.0.1".to_string(),
            post_check_host: String::new(),
            strict_nginx_reload: false,
        }
    }

    /// Build arg list for inspection (no subprocess).
    fn certonly_argv_synthetic(
        config: &SslProcessConfig,
        email: &str,
        domains: &[String],
        mode: SslModeResolved,
        webroot: Option<&str>,
    ) -> Result<Vec<String>, String> {
        if domains.is_empty() {
            return Err("no domains".into());
        }
        let (_prog, mut args) = super::certbot_argv_base(config);
        args.extend(
            [
                "certonly".to_string(),
                "--non-interactive".to_string(),
                "--agree-tos".to_string(),
                "-m".to_string(),
                email.to_string(),
            ]
            .into_iter(),
        );
        args.extend(config.certbot_extra_args.iter().cloned());
        match mode {
            SslModeResolved::Nginx => args.push("--nginx".to_string()),
            SslModeResolved::Standalone => args.push("--standalone".to_string()),
            SslModeResolved::Webroot => {
                let w = webroot
                    .or(config.webroot_path.as_deref())
                    .ok_or("webroot")?;
                args.push("--webroot".to_string());
                args.push("-w".to_string());
                args.push(w.to_string());
            }
            SslModeResolved::Dns => {}
        }
        for d in domains {
            args.push("-d".to_string());
            args.push(d.clone());
        }
        Ok(args)
    }

    #[test]
    fn argv_contains_nginx() {
        let c = test_config();
        let a = certonly_argv_synthetic(&c, "a@b", &["x.com".to_string()], SslModeResolved::Nginx, None)
            .unwrap();
        assert!(a.contains(&"--nginx".to_string()));
    }
}
