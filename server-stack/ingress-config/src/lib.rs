//! Build [sing-box](https://sing-box.sagernet.org/) configuration from `grpc_proxy_session` rows.
//!
//! **Limits bridge:** outbound is `direct`; DB traffic caps apply to the gRPC `ProxyTunnel` path only.
//! Revoking a session and re-running [`build_singbox_config`] (or `ingress-manager`) removes its inbound.

use deploy_db::GrpcProxySessionRow;
use serde_json::{json, Value};

pub const SINGBOX_TEMPLATE_VERSION: u32 = 1;

/// Template version stored in `grpc_proxy_session.ingress_template_version` (config generator revisions).
#[must_use]
pub fn singbox_template_version() -> u32 {
    SINGBOX_TEMPLATE_VERSION
}

fn proto_to_type(p: i16) -> Option<&'static str> {
    match p {
        1 => Some("vless"),
        2 => Some("vmess"),
        3 => Some("trojan"),
        4 => Some("shadowsocks"),
        5 => Some("socks"),
        6 => Some("hysteria2"),
        _ => None,
    }
}

/// Overlay `fixed` keys on top of `inner` (fixed wins for listen/tag/type).
fn overlay_inbound(inner: Value, fixed: serde_json::Map<String, Value>) -> Value {
    match inner {
        Value::Object(mut m) => {
            for (k, v) in fixed {
                m.insert(k, v);
            }
            Value::Object(m)
        }
        _ => Value::Object(fixed),
    }
}

/// Options for the generated document (listen address, log level).
#[derive(Debug, Clone)]
pub struct SingboxBuildOptions {
    pub listen_address: String,
    pub log_level: String,
}

impl Default for SingboxBuildOptions {
    fn default() -> Self {
        Self {
            listen_address: std::env::var("INGRESS_LISTEN_ADDRESS")
                .unwrap_or_else(|_| "0.0.0.0".to_string()),
            log_level: std::env::var("INGRESS_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        }
    }
}

/// One sing-box inbound from a session row (`ingress_protocol` set, not revoked).
pub fn inbound_from_row(row: &GrpcProxySessionRow, opts: &SingboxBuildOptions) -> Result<Value, String> {
    let proto = row.ingress_protocol.ok_or_else(|| {
        "ingress_protocol is NULL".to_string()
    })?;
    let typ = proto_to_type(proto).ok_or_else(|| format!("unknown ingress_protocol {proto}"))?;
    let port = row
        .ingress_listen_port
        .ok_or_else(|| "ingress_listen_port is NULL".to_string())?;
    
    if port <= 0 || port > 65535 {
        return Err("invalid ingress_listen_port".to_string());
    }
    let cfg = row
        .ingress_config_json
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "ingress_config_json is empty".to_string())?;
    let inner: Value =
        serde_json::from_str(cfg).map_err(|e| format!("ingress_config_json: {e}"))?;
    let tag = format!("in-{}", row.session_id.replace('-', ""));
    let mut base = serde_json::Map::new();
    base.insert("type".into(), json!(typ));
    base.insert("tag".into(), json!(tag));
    base.insert("listen".into(), json!(&opts.listen_address));
    base.insert("listen_port".into(), json!(port));
    if let Some(ref tls) = row.ingress_tls_json {
        let t = tls.trim();
        if !t.is_empty() {
            let v: Value = serde_json::from_str(t).map_err(|e| format!("ingress_tls_json: {e}"))?;
            base.insert("tls".into(), v);
        }
    }
    Ok(overlay_inbound(inner, base))
}

/// Full sing-box JSON with `inbounds` for all active ingress sessions, `outbounds` = direct.
pub fn build_singbox_config(
    rows: &[GrpcProxySessionRow],
    opts: &SingboxBuildOptions,
) -> Result<Value, String> {
    let mut inbounds = Vec::new();
    for r in rows {
        if r.revoked {
            continue;
        }
        if r.ingress_protocol.is_none() {
            continue;
        }
        inbounds.push(inbound_from_row(r, opts)?);
    }
    Ok(json!({
        "log": { "level": opts.log_level },
        "inbounds": inbounds,
        "outbounds": [
            { "type": "direct", "tag": "direct" }
        ],
        "route": {
            "final": "direct"
        }
    }))
}

pub mod limits_bridge {
    /// Explains that per-session traffic limits from `grpc_proxy_session` are enforced on the
    /// deploy-server `ProxyTunnel` path; sing-box ingress uses a `direct` outbound unless extended.
    pub const DIRECT_OUTBOUND_LIMITS_NOTE: &str =
        "Pirate DB traffic/active-time limits apply to gRPC tunnel usage. Full ingress (sing-box) uses direct outbound; revoke sessions via control-api to drop inbounds.";
}
