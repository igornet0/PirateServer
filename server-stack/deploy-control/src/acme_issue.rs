//! Optional Let’s Encrypt issuance from control-api (`set_ssl` + `issue_certificate_if_missing`).
use crate::types::NginxActionBody;
use crate::ControlError;
use std::path::Path;
use std::process::Command;

fn env_trim(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SslMode {
    Nginx,
    Standalone,
    Webroot,
    Dns,
}

fn ssl_mode_from_env() -> SslMode {
    match env_trim("SSL_MODE")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "standalone" => SslMode::Standalone,
        "webroot" => SslMode::Webroot,
        "dns" => SslMode::Dns,
        _ => SslMode::Nginx,
    }
}

fn certbot_bin() -> String {
    env_trim("SSL_CERTBOT_BIN").unwrap_or_else(|| "certbot".to_string())
}

fn extra_certbot_args() -> Vec<String> {
    env_trim("SSL_CERTBOT_EXTRA_ARGS")
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

/// Run `sudo -n certbot …` or `certbot …` depending on `SSL_USE_SUDO` and euid.
fn certbot_output(args: &[String]) -> Result<std::process::Output, ControlError> {
    let use_sudo = env_bool("SSL_USE_SUDO", true);
    #[cfg(unix)]
    let need_sudo = {
        let uid = unsafe { libc::geteuid() };
        uid != 0 && use_sudo
    };
    #[cfg(not(unix))]
    let need_sudo = use_sudo;

    let bin = certbot_bin();
    let mut cmd = if need_sudo {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(&bin);
        c
    } else {
        Command::new(&bin)
    };
    cmd.args(args);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.output()
        .map_err(|e| ControlError::NginxOp(format!("certbot spawn failed: {e}")))
}

fn merged_cmd_output(o: &std::process::Output) -> String {
    [
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr),
    ]
    .concat()
}

/// Tokens on certbot's `Domains:` line (commas and/or spaces).
/// Paths printed by certbot after successful `certonly` (works when unprivileged `stat` on `/etc/letsencrypt` fails).
fn parse_certbot_certonly_saved_paths(text: &str) -> Option<(String, String)> {
    let mut fullchain: Option<String> = None;
    let mut privkey: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        let low = line.to_ascii_lowercase();
        if let Some(idx) = low.find("certificate is saved at:") {
            let path = line[idx + "certificate is saved at:".len()..].trim();
            if path.starts_with('/') {
                fullchain = Some(path.to_string());
            }
        }
        if let Some(idx) = low.find("key is saved at:") {
            let path = line[idx + "key is saved at:".len()..].trim();
            if path.starts_with('/') {
                privkey = Some(path.to_string());
            }
        }
    }
    match (fullchain, privkey) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

fn domain_tokens_contain(rest: &str, want: &str) -> bool {
    let want = want.trim().to_ascii_lowercase();
    if want.is_empty() {
        return false;
    }
    rest.split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| {
            s.trim()
                .trim_end_matches(',')
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty())
        .any(|d| d == want)
}

fn need_sudo_for_privileged_paths() -> bool {
    let use_sudo = env_bool("SSL_USE_SUDO", true);
    #[cfg(unix)]
    {
        use_sudo && unsafe { libc::geteuid() } != 0
    }
    #[cfg(not(unix))]
    {
        use_sudo
    }
}

/// Single path check: `Path::is_file` first; if false and the process is unprivileged with `SSL_USE_SUDO=1`, run `sudo -n test -f`.
/// Used for nginx preflight and PEM pair checks (typical under `/etc/letsencrypt/...` where unprivileged `stat` fails).
pub(crate) fn privileged_path_is_file(path: &str) -> bool {
    if Path::new(path).is_file() {
        return true;
    }
    if !need_sudo_for_privileged_paths() {
        return false;
    }
    Command::new("sudo")
        .args(["-n", "test", "-f", path])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `Path::is_file` is false for unprivileged users on typical `/etc/letsencrypt` permissions; mirror certbot invocation with `sudo test`.
pub(crate) fn letsencrypt_pem_pair_present(cert_path: &str, key_path: &str) -> bool {
    pem_pair_present(cert_path, key_path)
}

fn pem_pair_present(fc: &str, pk: &str) -> bool {
    privileged_path_is_file(fc) && privileged_path_is_file(pk)
}

/// Parse `certbot certificates` for a block covering `domain`.
fn parse_certbot_certificates_for_domain(text: &str, domain: &str) -> Option<(String, String)> {
    let want = domain.trim().to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    let mut in_block = false;
    let mut cert_name_matches = false;
    let mut block_matches = false;
    let mut cert_path: Option<String> = None;
    let mut key_path: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Certificate Name:") {
            in_block = true;
            let cn = t
                .strip_prefix("Certificate Name:")
                .map(str::trim)
                .unwrap_or("")
                .to_ascii_lowercase();
            cert_name_matches = cn == want;
            block_matches = cert_name_matches;
            cert_path = None;
            key_path = None;
            continue;
        }
        if !in_block {
            continue;
        }
        if t.starts_with("Domains:") {
            let rest = t.strip_prefix("Domains:").unwrap_or("").trim();
            let domain_matches = domain_tokens_contain(rest, &want);
            block_matches = cert_name_matches || domain_matches;
            cert_path = None;
            key_path = None;
            continue;
        }
        if block_matches && t.starts_with("Certificate Path:") {
            cert_path = t
                .strip_prefix("Certificate Path:")
                .map(str::trim)
                .map(String::from);
        }
        if block_matches && t.starts_with("Private Key Path:") {
            key_path = t
                .strip_prefix("Private Key Path:")
                .map(str::trim)
                .map(String::from);
            if let (Some(c), Some(k)) = (&cert_path, &key_path) {
                return Some((c.clone(), k.clone()));
            }
        }
    }
    None
}

fn resolve_letsencrypt_paths_after_issue(domain: &str) -> Result<(String, String), ControlError> {
    let dom = domain.trim();
    let live = Path::new("/etc/letsencrypt/live").join(dom);
    let fc = live.join("fullchain.pem");
    let pk = live.join("privkey.pem");
    let fc_s = fc.to_string_lossy().into_owned();
    let pk_s = pk.to_string_lossy().into_owned();
    if pem_pair_present(&fc_s, &pk_s) {
        return Ok((fc_s, pk_s));
    }

    let list_args = vec!["certificates".to_string()];
    let out = certbot_output(&list_args)?;
    if !out.status.success() {
        return Err(ControlError::NginxOp(format!(
            "certbot certificates failed (exit {:?}): {}",
            out.status.code(),
            merged_cmd_output(&out).trim()
        )));
    }
    let text = merged_cmd_output(&out);
    if let Some((c, k)) = parse_certbot_certificates_for_domain(&text, dom) {
        if pem_pair_present(&c, &k) {
            return Ok((c, k));
        }
    }

    // Fallback: scan live subdirs (e.g. example.com-0001); read_dir often fails without root — use sudo ls.
    let mut candidates: Vec<std::path::PathBuf> = vec![];
    let base = Path::new("/etc/letsencrypt/live");
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                candidates.push(p);
            }
        }
    }
    if candidates.is_empty() && need_sudo_for_privileged_paths() {
        let o = Command::new("sudo")
            .args(["-n", "ls", "-1", "/etc/letsencrypt/live"])
            .output();
        if let Ok(o) = o {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let name = line.trim();
                    if name.is_empty() || name.starts_with('.') {
                        continue;
                    }
                    candidates.push(base.join(name));
                }
            }
        }
    }
    for p in candidates {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name != dom && !name.starts_with(&format!("{dom}-")) {
            continue;
        }
        let fc = p.join("fullchain.pem");
        let pk = p.join("privkey.pem");
        let fc_s = fc.to_string_lossy().into_owned();
        let pk_s = pk.to_string_lossy().into_owned();
        if pem_pair_present(&fc_s, &pk_s) {
            return Ok((fc_s, pk_s));
        }
    }

    Err(ControlError::NginxOp(format!(
        "certbot finished but could not verify fullchain.pem+privkey.pem for domain '{dom}' (if files exist as root, ensure control-api can run `sudo -n test -f` on those paths and `sudo -n certbot certificates`; check certbot stdout/stderr for dry-run or failure)"
    )))
}

/// Issue a certificate for `domain` when default LE paths are missing. Uses same env contract as deploy-server SSL.
pub fn issue_letsencrypt_certificate(domain: &str, action: &NginxActionBody) -> Result<(String, String), ControlError> {
    let dom = domain.trim();
    if dom.is_empty() || dom == "_" || dom.starts_with("*.") {
        return Err(ControlError::NginxOp(
            "cannot issue certificate: need a concrete domain (not wildcard-only)".into(),
        ));
    }

    let mode = ssl_mode_from_env();
    if mode == SslMode::Dns {
        return Err(ControlError::NginxOp(
            "SSL_MODE=dns: issue certificate via gRPC SslCreate or switch SSL_MODE to nginx/webroot/standalone for set_ssl auto-issue"
                .into(),
        ));
    }

    let email = action
        .acme_email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| env_trim("SSL_EMAIL"));

    let staging = action.acme_staging.unwrap_or_else(|| env_bool("SSL_STAGING", false));
    let dry_run = action.acme_dry_run.unwrap_or(false);

    let mut args: Vec<String> = vec![
        "certonly".into(),
        "--non-interactive".into(),
        "--agree-tos".into(),
    ];

    if let Some(ref m) = email {
        args.push("-m".into());
        args.push(m.clone());
    } else {
        args.push("--register-unsafely-without-email".into());
    }

    if dry_run {
        args.push("--dry-run".into());
    }
    if staging {
        args.push("--test-cert".into());
    }

    args.extend(extra_certbot_args());

    match mode {
        SslMode::Nginx => {
            args.push("--nginx".into());
        }
        SslMode::Standalone => {
            args.push("--standalone".into());
        }
        SslMode::Webroot => {
            let w = env_trim("SSL_WEBROOT").ok_or_else(|| {
                ControlError::NginxOp(
                    "SSL_MODE=webroot requires SSL_WEBROOT=/path for certbot".into(),
                )
            })?;
            args.push("--webroot".into());
            args.push("-w".into());
            args.push(w);
        }
        SslMode::Dns => unreachable!(),
    }

    args.push("-d".into());
    args.push(dom.to_string());

    let out = certbot_output(&args)?;
    let merged = merged_cmd_output(&out);
    if !out.status.success() {
        return Err(ControlError::NginxOp(format!(
            "certbot certonly failed (exit {:?}): {}",
            out.status.code(),
            merged.trim()
        )));
    }

    if let Some(pair) = parse_certbot_certonly_saved_paths(&merged) {
        return Ok(pair);
    }

    resolve_letsencrypt_paths_after_issue(dom)
}

#[cfg(test)]
mod tests {
    use super::{
        domain_tokens_contain, parse_certbot_certificates_for_domain,
        parse_certbot_certonly_saved_paths,
    };

    #[test]
    fn parse_certbot_listing() {
        let sample = r"
Found the following certs:
  Certificate Name: furry.agent-trade.ru
    Domains: furry.agent-trade.ru
    Certificate Path: /etc/letsencrypt/live/furry.agent-trade.ru/fullchain.pem
    Private Key Path: /etc/letsencrypt/live/furry.agent-trade.ru/privkey.pem
";
        let p = parse_certbot_certificates_for_domain(sample, "furry.agent-trade.ru");
        assert!(p.is_some());
        let (c, k) = p.unwrap();
        assert!(c.contains("fullchain.pem"));
        assert!(k.contains("privkey.pem"));
    }

    #[test]
    fn parse_domains_with_commas() {
        let sample = r"
  Certificate Name: x
    Domains: furry.agent-trade.ru, www.furry.agent-trade.ru
    Certificate Path: /etc/letsencrypt/live/x/fullchain.pem
    Private Key Path: /etc/letsencrypt/live/x/privkey.pem
";
        let p = parse_certbot_certificates_for_domain(sample, "furry.agent-trade.ru");
        assert!(p.is_some());
    }

    #[test]
    fn domain_tokens_commas() {
        assert!(domain_tokens_contain("a.example.com, b.example.com", "a.example.com"));
    }

    #[test]
    fn parse_certonly_output_lines() {
        let sample = r"
Successfully received certificate.
Certificate is saved at: /etc/letsencrypt/live/furry.agent-trade.ru/fullchain.pem
Key is saved at: /etc/letsencrypt/live/furry.agent-trade.ru/privkey.pem
";
        let p = parse_certbot_certonly_saved_paths(sample);
        assert_eq!(
            p,
            Some((
                "/etc/letsencrypt/live/furry.agent-trade.ru/fullchain.pem".into(),
                "/etc/letsencrypt/live/furry.agent-trade.ru/privkey.pem".into()
            ))
        );
    }
}
