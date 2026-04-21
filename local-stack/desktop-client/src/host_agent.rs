//! Out-of-band HTTP host-agent (health, status, reboot, server-stack upload).

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::time::Duration;

fn normalize_base(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

fn auth_header(token: &str) -> Result<HeaderMap, String> {
    let mut h = HeaderMap::new();
    let v = format!("Bearer {}", token.trim());
    h.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&v).map_err(|e| e.to_string())?,
    );
    Ok(h)
}

/// GET /health — no auth.
pub fn host_agent_health_json(base_url: &str) -> Result<String, String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("host agent base URL is empty".into());
    }
    let url = format!("{}/health", base);
    let c = client()?;
    let r = c.get(&url).send().map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, text.chars().take(500).collect::<String>()));
    }
    Ok(text)
}

/// GET /v1/status
pub fn host_agent_status_json(base_url: &str, token: &str) -> Result<String, String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("host agent base URL is empty".into());
    }
    if token.trim().is_empty() {
        return Err("host agent token is empty".into());
    }
    let url = format!("{}/v1/status", base);
    let c = client()?;
    let r = c
        .get(&url)
        .headers(auth_header(token)?)
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, text.chars().take(500).collect::<String>()));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("JSON: {e}"))?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct RebootBody {
    delay_sec: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// POST /v1/reboot
pub fn host_agent_reboot_json(
    base_url: &str,
    token: &str,
    delay_sec: u64,
    reason: Option<String>,
) -> Result<String, String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("host agent base URL is empty".into());
    }
    if token.trim().is_empty() {
        return Err("host agent token is empty".into());
    }
    let url = format!("{}/v1/reboot", base);
    let body = RebootBody {
        delay_sec,
        reason,
    };
    let c = client()?;
    let r = c
        .post(&url)
        .headers(auth_header(token)?)
        .json(&body)
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, text.chars().take(500).collect::<String>()));
    }
    Ok(text)
}

/// POST /v1/server-stack with tarball body (same bundle as gRPC OTA).
pub fn host_agent_upload_server_stack(
    base_url: &str,
    token: &str,
    tarball_path: &std::path::Path,
    version: &str,
) -> Result<String, String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("host agent base URL is empty".into());
    }
    if token.trim().is_empty() {
        return Err("host agent token is empty".into());
    }
    let mut file = fs::File::open(tarball_path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let digest = Sha256::digest(&buf);
    let sha_hex = hex::encode(digest);

    let url = format!("{}/v1/server-stack", base);
    let c = client()?;
    let mut headers = auth_header(token)?;
    headers.insert(
        "X-Pirate-Version",
        HeaderValue::from_str(version.trim()).map_err(|e| e.to_string())?,
    );
    headers.insert(
        "X-Pirate-Sha256",
        HeaderValue::from_str(&sha_hex).map_err(|e| e.to_string())?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/gzip"));

    let r = c
        .post(&url)
        .headers(headers)
        .body(buf)
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, text.chars().take(800).collect::<String>()));
    }
    Ok(text)
}
