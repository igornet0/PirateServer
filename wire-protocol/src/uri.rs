//! Parse `vless://`, `vmess://`, `trojan://` into normalized [`WireParams`] + mode hints.

use crate::params::{WireMode, WireParams};
use crate::WireError;
use base64::Engine;
use serde_json::Value as Json;

#[derive(Debug, Clone)]
pub struct ParsedSubscription {
    pub mode: WireMode,
    pub params: WireParams,
    /// Original outbound host from URI (informational; Pirate uses gRPC, not direct TCP).
    pub remote_host: String,
    pub remote_port: u16,
}

fn parse_host_port(rest: &str) -> Option<(String, u16)> {
    let (host, port_s) = rest.rsplit_once(':')?;
    let port: u16 = port_s.parse().ok()?;
    let host = if host.starts_with('[') && host.ends_with(']') {
        host[1..host.len() - 1].to_string()
    } else {
        host.to_string()
    };
    Some((host, port))
}

/// Parse subscription URI; `WireMode` is inferred from scheme.
pub fn parse_subscription_uri(uri: &str) -> Result<ParsedSubscription, WireError> {
    let t = uri.trim();
    if t.starts_with("vless://") {
        return parse_vless(t);
    }
    if t.starts_with("vmess://") {
        return parse_vmess(t);
    }
    if t.starts_with("trojan://") {
        return parse_trojan(t);
    }
    Err(WireError::Parse("unsupported URI scheme (use vless://, vmess://, trojan://)".into()))
}

fn parse_vless(t: &str) -> Result<ParsedSubscription, WireError> {
    let rest = &t["vless://".len()..];
    let (userinfo, after_at) = rest
        .split_once('@')
        .ok_or_else(|| WireError::Parse("vless: missing @".into()))?;
    let uuid = userinfo.trim().to_string();
    let path_part = after_at.split(['?', '#']).next().unwrap_or(after_at);
    let (host, port) = parse_host_port(path_part)
        .ok_or_else(|| WireError::Parse("vless: bad host:port".into()))?;
    Ok(ParsedSubscription {
        mode: WireMode::Vless,
        params: WireParams {
            uuid: Some(uuid),
            password: None,
            flow: None,
        },
        remote_host: host,
        remote_port: port,
    })
}

fn parse_trojan(t: &str) -> Result<ParsedSubscription, WireError> {
    let rest = &t["trojan://".len()..];
    let (userinfo, after_at) = rest
        .split_once('@')
        .ok_or_else(|| WireError::Parse("trojan: missing @".into()))?;
    let password = userinfo.trim().to_string();
    let path_part = after_at.split(['?', '#']).next().unwrap_or(after_at);
    let (host, port) = parse_host_port(path_part)
        .ok_or_else(|| WireError::Parse("trojan: bad host:port".into()))?;
    Ok(ParsedSubscription {
        mode: WireMode::Trojan,
        params: WireParams {
            uuid: None,
            password: Some(password),
            flow: None,
        },
        remote_host: host,
        remote_port: port,
    })
}

fn parse_vmess(t: &str) -> Result<ParsedSubscription, WireError> {
    let b64 = t.trim_start_matches("vmess://");
    let pad = (4 - b64.len() % 4) % 4;
    let padded = format!("{}{}", b64, "=".repeat(pad));
    let raw = base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .map_err(|e| WireError::Parse(format!("vmess base64: {e}")))?;
    let j: Json = serde_json::from_slice(&raw)
        .map_err(|e| WireError::Parse(format!("vmess json: {e}")))?;
    let add = j
        .get("add")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let port = j
        .get("port")
        .and_then(|v| v.as_u64())
        .unwrap_or(443) as u16;
    let id = j
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Err(WireError::Parse("vmess: missing id".into()));
    }
    Ok(ParsedSubscription {
        mode: WireMode::Vmess,
        params: WireParams {
            uuid: Some(id),
            password: None,
            flow: None,
        },
        remote_host: add,
        remote_port: port,
    })
}

impl WireParams {
    pub fn to_json_string(&self) -> Result<String, WireError> {
        serde_json::to_string(self).map_err(|e| WireError::BadConfig(e.to_string()))
    }
}
