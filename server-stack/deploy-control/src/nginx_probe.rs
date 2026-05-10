//! HTTPS smoke probe to localhost with SNI via `curl` (verifies cert chain + response status through nginx).
use std::process::Command;
use std::time::Duration;

/// Outcome of `curl` against `https://{host}{path}` with `--resolve` to the loopback address.
#[derive(Debug, Clone)]
pub struct HttpsLocalProbe {
    /// True when we consider TLS + HTTP successful for deployment (2xx, 3xx, 4xx, or 401/403).
    pub ok: bool,
    /// HTTP status if parsed (0 = unknown)
    pub http_status: u32,
    pub curl_exit: i32,
    /// Short classification: `ok`, `upstream_5xx`, `tls_name_mismatch`, `connect_failed`, etc.
    pub classified: String,
    pub detail: String,
    pub tls_effective: bool,
    /// SNI / Host used for the request (echo of input host when concrete).
    pub probe_host: String,
}

fn classify_curl_failure(exit_code: i32, stderr: &str) -> &'static str {
    match exit_code {
        60 => "tls_name_mismatch",
        51 => "tls_peer_cert",
        58 => "tls_cert_problem",
        35 => "tls_handshake_failed",
        7 => "connection_refused",
        28 => "timeout",
        6 => "dns_failed",
        _ => {
            let s = stderr.to_ascii_lowercase();
            if s.contains("no alternative certificate subject name")
                || s.contains("ssl: certificate subject name")
                || (s.contains("certificate") && s.contains("does not match"))
            {
                return "tls_name_mismatch";
            }
            if s.contains("connection refused") {
                return "connection_refused";
            }
            if s.contains("timed out") || s.contains("timeout") || s.contains("operation timed out") {
                return "timeout";
            }
            if s.contains("could not resolve host") {
                return "dns_failed";
            }
            "connect_failed"
        }
    }
}

/// `openssl x509 -checkhost` against a PEM file (exit 0 when hostname matches SAN/CN).
/// Retries with `sudo -n openssl` when the direct read fails (typical for `/etc/letsencrypt`).
pub fn openssl_x509_checkhost_pem(cert_path: &str, host: &str) -> Result<(), String> {
    let cert = cert_path.trim();
    let h = host.trim();
    if cert.is_empty() || h.is_empty() {
        return Err("cert path and host required for -checkhost".into());
    }
    let run = |sudo: bool| {
        if sudo {
            Command::new("sudo")
                .args(["-n", "openssl", "x509", "-in", cert, "-noout", "-checkhost", h])
                .output()
        } else {
            Command::new("openssl")
                .args(["x509", "-in", cert, "-noout", "-checkhost", h])
                .output()
        }
    };
    let o = run(false).map_err(|e| format!("openssl: {e}"))?;
    let o = if o.status.success() {
        o
    } else {
        run(true).map_err(|e| format!("openssl (sudo): {e}"))?
    };
    if o.status.success() {
        return Ok(());
    }
    let msg = [String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)].concat();
    Err(if msg.trim().is_empty() {
        format!("openssl x509 -checkhost failed (exit {:?})", o.status.code())
    } else {
        msg.trim().to_string()
    })
}

/// Probes `https://host:port` by resolving `host` to `loopback` (SNI and cert name stay `host`).
/// Requires `curl` in PATH.
pub fn https_probe_localhost_resolve(
    host: &str,
    path: &str,
    port: u16,
    loopback: &str,
) -> HttpsLocalProbe {
    let h = host.trim();
    let probe_host = h.to_string();
    if h.is_empty() || h.starts_with("*.") {
        return HttpsLocalProbe {
            ok: true,
            http_status: 0,
            curl_exit: 0,
            classified: "skipped_wildcard_or_empty".into(),
            detail: "no concrete host for SNI/HTTP check".into(),
            tls_effective: false,
            probe_host,
        };
    }
    let p = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let which = Command::new("which").arg("curl").output();
    if which.map(|o| !o.status.success()).unwrap_or(true) {
        return HttpsLocalProbe {
            ok: true,
            http_status: 0,
            curl_exit: -1,
            classified: "curl_unavailable".into(),
            detail: "curl not found; install curl for HTTPS post-checks".into(),
            tls_effective: false,
            probe_host,
        };
    }
    let resolve = format!("{h}:{port}:{loopback}");
    let url = format!("https://{h}:{port}{p}");
    let out = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--connect-timeout",
            "5",
            "--max-time",
            "20",
            "--resolve",
            &resolve,
            &url,
        ])
        .output();
    match out {
        Ok(o) => {
            let code_s = String::from_utf8_lossy(&o.stdout);
            let http_status = code_s.trim().parse::<u32>().unwrap_or(0);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let exit_code = o.status.code().unwrap_or(-1);
            let merged = if stderr.is_empty() {
                format!("code={code_s} exit={exit_code}")
            } else {
                format!("{stderr} (http_code={code_s}, exit={exit_code})")
            };
            if !o.status.success() {
                let classified = classify_curl_failure(exit_code, &stderr).to_string();
                return HttpsLocalProbe {
                    ok: false,
                    http_status,
                    curl_exit: exit_code,
                    classified,
                    detail: merged,
                    tls_effective: false,
                    probe_host,
                };
            }
            // 5xx: bad gateway / upstream — primary regression we guard against
            if (500..600).contains(&http_status) {
                return HttpsLocalProbe {
                    ok: false,
                    http_status,
                    curl_exit: 0,
                    classified: "upstream_5xx".into(),
                    detail: merged,
                    tls_effective: true,
                    probe_host,
                };
            }
            HttpsLocalProbe {
                ok: true,
                http_status,
                curl_exit: 0,
                classified: "ok".into(),
                detail: merged,
                tls_effective: true,
                probe_host,
            }
        }
        Err(e) => HttpsLocalProbe {
            ok: false,
            http_status: 0,
            curl_exit: -1,
            classified: "curl_exec_failed".into(),
            detail: e.to_string(),
            tls_effective: false,
            probe_host,
        },
    }
}

fn probe_failure_transient(classified: &str) -> bool {
    matches!(
        classified,
        "connect_failed" | "connection_refused" | "timeout" | "upstream_5xx" | "dns_failed"
    )
}

/// Same as [`https_probe_localhost_resolve`] with short backoff retries (nginx reload settle).
pub fn https_probe_localhost_resolve_with_retries(
    host: &str,
    path: &str,
    port: u16,
    loopback: &str,
    max_attempts: u32,
    base_delay_ms: u64,
) -> HttpsLocalProbe {
    let mut last = https_probe_localhost_resolve(host, path, port, loopback);
    if last.ok || last.classified == "curl_unavailable" || last.classified == "skipped_wildcard_or_empty"
    {
        return last;
    }
    for attempt in 1..max_attempts {
        if !probe_failure_transient(&last.classified) {
            break;
        }
        let delay = base_delay_ms.saturating_mul(u64::from(attempt));
        std::thread::sleep(Duration::from_millis(delay.max(1)));
        last = https_probe_localhost_resolve(host, path, port, loopback);
        if last.ok || last.classified == "curl_unavailable" {
            break;
        }
    }
    last
}

/// Whether a failed HTTPS probe should trigger nginx config rollback (`set_ssl`).
pub fn https_probe_failure_warrants_rollback(classified: &str) -> bool {
    !matches!(
        classified,
        "tls_name_mismatch"
            | "tls_peer_cert"
            | "tls_cert_problem"
            | "skipped_wildcard_or_empty"
            | "curl_unavailable"
            | "skipped"
            | "ok"
    )
}

#[cfg(test)]
mod tests {
    use super::{classify_curl_failure, https_probe_localhost_resolve};

    #[test]
    fn wildcard_host_skips() {
        let r = https_probe_localhost_resolve("*.ex.com", "/", 443, "127.0.0.1");
        assert!(r.ok);
        assert_eq!(r.classified, "skipped_wildcard_or_empty");
    }

    #[test]
    fn classify_exit_60() {
        assert_eq!(
            classify_curl_failure(60, "SSL: no alternative certificate"),
            "tls_name_mismatch"
        );
    }

    #[test]
    fn classify_exit_7() {
        assert_eq!(classify_curl_failure(7, ""), "connection_refused");
    }
}
