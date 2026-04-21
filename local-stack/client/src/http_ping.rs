//! HTTP ping to control-api `GET /api/v1/ping` (direct or via HTTP CONNECT proxy).

use reqwest::Proxy;
use serde::Serialize;
use std::time::Instant;

const RTT_RUNS: usize = 3;
const SPEED_RUNS: usize = 3;
const PING_PATH: &str = "/api/v1/ping";

/// Trim and strip trailing `/` from control-api base (scheme + host[:port], optional path segment).
pub fn normalize_http_base(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

fn ping_url_json(base: &str) -> String {
    format!("{}{}", base, PING_PATH)
}

fn ping_url_bytes(base: &str, bytes: u64) -> String {
    format!("{}{}?bytes={}", base, PING_PATH, bytes)
}

fn build_client(
    proxy_url: Option<&str>,
    timeout: std::time::Duration,
) -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>> {
    let mut b = reqwest::Client::builder()
        .timeout(timeout)
        .pool_max_idle_per_host(0);
    if let Some(p) = proxy_url {
        let p = p.trim();
        if !p.is_empty() {
            b = b.proxy(Proxy::http(p)?);
        }
    }
    Ok(b.build()?)
}

async fn fetch_json_ping(
    client: &reqwest::Client,
    url: &str,
) -> Result<(reqwest::StatusCode, f64), reqwest::Error> {
    let t0 = Instant::now();
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let _ = resp.bytes().await?;
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    Ok((status, elapsed_ms))
}

async fn fetch_bytes_ping(
    client: &reqwest::Client,
    url: &str,
) -> Result<(reqwest::StatusCode, u64, f64), reqwest::Error> {
    let t0 = Instant::now();
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    let n = body.len() as u64;
    let elapsed_s = t0.elapsed().as_secs_f64();
    Ok((status, n, elapsed_s))
}

#[derive(Debug, Clone)]
pub struct HttpPingOptions {
    pub http_base: String,
    pub proxy_url: Option<String>,
    pub download_bytes: u64,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpPingJson {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping_url: Option<String>,
    pub via_proxy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_min_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_mbps: Option<f64>,
}

pub async fn run_http_ping(opts: HttpPingOptions) -> HttpPingJson {
    let base = normalize_http_base(&opts.http_base);
    if base.is_empty() {
        return HttpPingJson {
            ok: false,
            error: Some("http-url is empty".into()),
            ping_url: None,
            via_proxy: false,
            rtt_min_ms: None,
            rtt_avg_ms: None,
            download_bytes: None,
            download_mbps: None,
        };
    }

    let timeout = std::time::Duration::from_secs(opts.timeout_secs.max(1));
    let proxy_ref = opts.proxy_url.as_deref();
    let via_proxy = proxy_ref.is_some_and(|s| !s.trim().is_empty());

    let client = match build_client(
        proxy_ref.filter(|s| !s.trim().is_empty()),
        timeout,
    ) {
        Ok(c) => c,
        Err(e) => {
            return HttpPingJson {
                ok: false,
                error: Some(e.to_string()),
                ping_url: None,
                via_proxy,
                rtt_min_ms: None,
                rtt_avg_ms: None,
                download_bytes: None,
                download_mbps: None,
            };
        }
    };

    let json_url = ping_url_json(&base);

    let mut rtt_samples: Vec<f64> = Vec::with_capacity(RTT_RUNS);
    let mut ok = true;
    let mut err_msg: Option<String> = None;

    for _ in 0..RTT_RUNS {
        match fetch_json_ping(&client, &json_url).await {
            Ok((st, ms)) if st.is_success() => rtt_samples.push(ms),
            Ok((st, _)) => {
                ok = false;
                err_msg = Some(format!("HTTP {} on {}", st, json_url));
                break;
            }
            Err(e) => {
                ok = false;
                err_msg = Some(e.to_string());
                break;
            }
        }
    }

    let (rtt_min_ms, rtt_avg_ms) = if rtt_samples.is_empty() {
        (None, None)
    } else {
        let min = rtt_samples.iter().cloned().fold(f64::INFINITY, f64::min);
        let avg = rtt_samples.iter().sum::<f64>() / rtt_samples.len() as f64;
        (Some(min), Some(avg))
    };

    let mut download_mbps: Option<f64> = None;
    let mut download_actual: Option<u64> = None;

    if ok && opts.download_bytes > 0 {
        let url = ping_url_bytes(&base, opts.download_bytes);
        let mut best_mbps = 0_f64;
        let mut last_err: Option<String> = None;
        for _ in 0..SPEED_RUNS {
            match fetch_bytes_ping(&client, &url).await {
                Ok((st, n, elapsed_s)) if st.is_success() && elapsed_s > 0.0 => {
                    download_actual = Some(n);
                    let mbps = (n as f64 * 8.0) / 1_000_000.0 / elapsed_s;
                    if mbps > best_mbps {
                        best_mbps = mbps;
                    }
                }
                Ok((st, n, _)) => {
                    last_err = Some(format!("HTTP {} ({} bytes)", st, n));
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        if best_mbps > 0.0 {
            download_mbps = Some(best_mbps);
        } else {
            ok = false;
            err_msg = last_err.or_else(|| Some("download ping failed".into()));
        }
    }

    HttpPingJson {
        ok,
        error: err_msg,
        ping_url: Some(json_url),
        via_proxy,
        rtt_min_ms,
        rtt_avg_ms,
        download_bytes: if opts.download_bytes > 0 {
            download_actual.or(Some(opts.download_bytes))
        } else {
            None
        },
        download_mbps,
    }
}

pub fn print_http_ping_human(out: &HttpPingJson) {
    if !out.ok {
        eprintln!(
            "ping failed: {}",
            out.error.as_deref().unwrap_or("unknown error")
        );
        return;
    }
    let url = out.ping_url.as_deref().unwrap_or("/api/v1/ping");
    println!(
        "pong: {} (RTT over {} runs: min≈{:.2} ms, avg≈{:.2} ms){}",
        url,
        RTT_RUNS,
        out.rtt_min_ms.unwrap_or(0.0),
        out.rtt_avg_ms.unwrap_or(0.0),
        if out.via_proxy {
            " [via HTTP proxy]"
        } else {
            ""
        }
    );
    if let (Some(n), Some(m)) = (out.download_bytes, out.download_mbps) {
        println!(
            "download: {} bytes (best of {} runs) ≈ {:.2} Mbps",
            n, SPEED_RUNS, m
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_trims_slash() {
        assert_eq!(
            normalize_http_base("https://ex.com/foo/"),
            "https://ex.com/foo"
        );
        assert_eq!(normalize_http_base("  http://h:8080  "), "http://h:8080");
    }

    #[test]
    fn ping_urls() {
        assert_eq!(
            ping_url_json("http://localhost:8080"),
            "http://localhost:8080/api/v1/ping"
        );
        assert_eq!(
            ping_url_bytes("http://localhost:8080", 1024),
            "http://localhost:8080/api/v1/ping?bytes=1024"
        );
    }
}
