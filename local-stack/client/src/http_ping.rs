//! HTTP ping to control-api `GET /api/v1/ping` (direct or via HTTP CONNECT proxy).
//!
//! `reqwest` applies `Proxy::http()` to `http://` destinations using a forward proxy request line,
//! not HTTP CONNECT. [`pirate board`](crate::board) only accepts CONNECT, so for `http://` URLs
//! with a proxy we open a TCP tunnel manually (CONNECT then cleartext HTTP on the tunnel).

use reqwest::Proxy;
use reqwest::Url;
use serde::Serialize;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const RTT_RUNS: usize = 3;
const SPEED_RUNS: usize = 3;
const PING_PATH: &str = "/api/v1/ping";

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

async fn read_until_double_crlf(
    stream: &mut TcpStream,
    max: usize,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, BoxErr> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        if buf.len() > max {
            return Err("HTTP headers too large".into());
        }
        let n = match tokio::time::timeout(timeout, stream.read(&mut tmp)).await {
            Err(_) => return Err("read timed out".into()),
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(0)) => return Err("connection closed before headers complete".into()),
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = headers_end(&buf) {
            return Ok(buf[..end].to_vec());
        }
    }
}

/// `http://` GET through an HTTP proxy that only supports CONNECT (e.g. `pirate board`).
async fn get_http_over_connect(
    proxy_url: &str,
    request_url: &str,
    timeout: std::time::Duration,
) -> Result<(reqwest::StatusCode, f64, u64), BoxErr> {
    let t0 = Instant::now();
    let proxy_u = Url::parse(proxy_url.trim())?;
    let target = Url::parse(request_url)?;
    if target.scheme() != "http" {
        return Err("internal: get_http_over_connect expects http:// target".into());
    }

    let ph = proxy_u.host_str().ok_or("proxy URL has no host")?;
    let pp = proxy_u
        .port()
        .or_else(|| match proxy_u.scheme() {
            "http" => Some(80u16),
            "https" => Some(443u16),
            _ => None,
        })
        .unwrap_or(3128u16);

    let authority = target.authority();
    if authority.is_empty() {
        return Err("request URL has no authority".into());
    }

    let mut stream = tokio::time::timeout(timeout, TcpStream::connect((ph, pp)))
        .await
        .map_err(|_| -> BoxErr { "connect to proxy timed out".into() })?
        .map_err(|e| -> BoxErr { e.into() })?;

    let connect_req = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    stream.write_all(connect_req.as_bytes()).await?;

    let connect_head = read_until_double_crlf(&mut stream, 64 * 1024, timeout).await?;
    let connect_text = std::str::from_utf8(&connect_head)?;
    let line1 = connect_text.split("\r\n").next().unwrap_or("");
    if !line1.starts_with("HTTP/1.") {
        return Err(format!("bad CONNECT response: {line1}").into());
    }
    let code = line1.split_whitespace().nth(1).unwrap_or("");
    if code != "200" {
        return Err(format!("CONNECT failed ({code}): {line1}").into());
    }

    let path = target.path();
    let path = if path.is_empty() { "/" } else { path };
    let path_q = match target.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    };
    let get_req = format!(
        "GET {path_q} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nUser-Agent: pirate-http-ping\r\n\r\n"
    );
    stream.write_all(get_req.as_bytes()).await?;

    let mut body_buf = Vec::new();
    let mut read_buf = [0u8; 8192];
    loop {
        let n = match tokio::time::timeout(timeout, stream.read(&mut read_buf)).await {
            Err(_) => return Err("read response timed out".into()),
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => n,
        };
        body_buf.extend_from_slice(&read_buf[..n]);
        if body_buf.len() > 512 * 1024 {
            break;
        }
    }

    let resp_head_end = headers_end(&body_buf).ok_or("invalid HTTP response (no header block)")?;
    let resp_head = std::str::from_utf8(&body_buf[..resp_head_end])?;
    let resp_line1 = resp_head.split("\r\n").next().unwrap_or("");
    let status_code: u16 = resp_line1
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let status = reqwest::StatusCode::from_u16(status_code)
        .unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
    let body_len = (body_buf.len().saturating_sub(resp_head_end)) as u64;
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    Ok((status, elapsed_ms, body_len))
}

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
            // `http` alone does not attach to https:// URLs; `board` uses CONNECT for both.
            b = b
                .proxy(Proxy::http(p)?)
                .proxy(Proxy::https(p)?);
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
    let use_connect_manual = via_proxy && base.starts_with("http://");

    let client_opt = if use_connect_manual {
        None
    } else {
        match build_client(
            proxy_ref.filter(|s| !s.trim().is_empty()),
            timeout,
        ) {
            Ok(c) => Some(c),
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
        }
    };

    let json_url = ping_url_json(&base);

    let mut rtt_samples: Vec<f64> = Vec::with_capacity(RTT_RUNS);
    let mut ok = true;
    let mut err_msg: Option<String> = None;

    for _ in 0..RTT_RUNS {
        let one = if use_connect_manual {
            match get_http_over_connect(proxy_ref.unwrap(), &json_url, timeout).await {
                Ok((st, ms, _)) if st.is_success() => Ok(ms),
                Ok((st, _, _)) => Err(format!("HTTP {} on {}", st, json_url)),
                Err(e) => Err(e.to_string()),
            }
        } else {
            match fetch_json_ping(client_opt.as_ref().unwrap(), &json_url).await {
                Ok((st, ms)) if st.is_success() => Ok(ms),
                Ok((st, _)) => Err(format!("HTTP {} on {}", st, json_url)),
                Err(e) => Err(e.to_string()),
            }
        };
        match one {
            Ok(ms) => rtt_samples.push(ms),
            Err(e) => {
                ok = false;
                err_msg = Some(e);
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
            let run = if use_connect_manual {
                match get_http_over_connect(proxy_ref.unwrap(), &url, timeout).await {
                    Ok((st, elapsed_ms, n)) if st.is_success() => {
                        let elapsed_s = (elapsed_ms / 1000.0).max(1e-9);
                        Ok((st, n, elapsed_s))
                    }
                    Ok((st, _, n)) => Err(format!("HTTP {} ({} bytes)", st, n)),
                    Err(e) => Err(e.to_string()),
                }
            } else {
                match fetch_bytes_ping(client_opt.as_ref().unwrap(), &url).await {
                    Ok((st, n, elapsed_s)) if st.is_success() => Ok((st, n, elapsed_s)),
                    Ok((st, n, _)) => Err(format!("HTTP {} ({} bytes)", st, n)),
                    Err(e) => Err(e.to_string()),
                }
            };
            match run {
                Ok((_st, n, elapsed_s)) if elapsed_s > 0.0 => {
                    download_actual = Some(n);
                    let mbps = (n as f64 * 8.0) / 1_000_000.0 / elapsed_s;
                    if mbps > best_mbps {
                        best_mbps = mbps;
                    }
                }
                Ok(_) => {
                    last_err = Some("HTTP success but zero elapsed".into());
                }
                Err(e) => last_err = Some(e),
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
    fn headers_end_finds_double_crlf() {
        let b = b"HTTP/1.1 200 OK\r\n\r\n";
        assert_eq!(headers_end(b), Some(b.len()));
        assert!(headers_end(b"HTTP/1.1 200 OK\r\n").is_none());
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
