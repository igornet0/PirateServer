//! Xray-core client config from `grpc_proxy_session` (public ingress on host).
//!
//! Maps sing-box-shaped `ingress_config_json` / `ingress_tls_json` to common Xray fields where possible.

use deploy_db::GrpcProxySessionRow;
use serde_json::{json, Value};

/// Public address clients use to reach the host ingress (DNS / TLS).
#[derive(Debug, Clone)]
pub struct XrayExportOptions {
    pub server_address: String,
    /// Optional SNI / Reality server name override (`DEPLOY_SUBSCRIPTION_TLS_SNI`).
    pub tls_server_name: Option<String>,
}

fn proto_to_xray_protocol(p: i16) -> Option<&'static str> {
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

fn merge_stream_settings(
    ingress_tls_json: Option<&str>,
    tls_sni_override: Option<&str>,
) -> Result<Value, String> {
    let mut base = json!({
        "network": "tcp",
        "security": "none"
    });
    let tls_raw = ingress_tls_json
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(t) = tls_raw {
        let v: Value = serde_json::from_str(t).map_err(|e| format!("ingress_tls_json: {e}"))?;
        if let Some(obj) = v.as_object() {
            let enabled = obj
                .get("enabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(true);
            if enabled {
                base["security"] = json!("tls");
                let mut tls_settings = json!({});
                if let Some(sn) = obj.get("server_name").and_then(|x| x.as_str()) {
                    tls_settings["serverName"] = json!(sn);
                } else if let Some(sn) = tls_sni_override {
                    tls_settings["serverName"] = json!(sn);
                }
                if let Some(fp) = obj.get("fingerprint").and_then(|x| x.as_str()) {
                    tls_settings["fingerprint"] = json!(fp);
                }
                if let Some(alpn) = obj.get("alpn").and_then(|x| x.as_array()) {
                    tls_settings["alpn"] = json!(alpn);
                }
                if let Some(u) = obj.get("utls") {
                    tls_settings["utls"] = u.clone();
                }
                if let Some(r) = obj.get("reality") {
                    base["security"] = json!("reality");
                    base["realitySettings"] = r.clone();
                } else if tls_settings != json!({}) {
                    base["tlsSettings"] = tls_settings;
                }
            }
        }
    } else if let Some(sn) = tls_sni_override {
        base["security"] = json!("tls");
        base["tlsSettings"] = json!({ "serverName": sn });
    }
    Ok(base)
}

fn outbound_from_row(
    row: &GrpcProxySessionRow,
    addr: &str,
    port: i32,
    stream: Value,
) -> Result<Value, String> {
    let proto = row
        .ingress_protocol
        .ok_or_else(|| "ingress_protocol is NULL".to_string())?;
    let x = proto_to_xray_protocol(proto).ok_or_else(|| format!("unknown ingress_protocol {proto}"))?;
    let cfg = row
        .ingress_config_json
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "ingress_config_json is empty".to_string())?;
    let inner: Value = serde_json::from_str(cfg).map_err(|e| format!("ingress_config_json: {e}"))?;

    match x {
        "vless" => {
            let uuid = inner
                .get("uuid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let flow = inner.get("flow").cloned().unwrap_or(json!(null));
            Ok(json!({
                "protocol": "vless",
                "settings": {
                    "vnext": [{
                        "address": addr,
                        "port": port,
                        "users": [{
                            "id": uuid,
                            "encryption": "none",
                            "flow": flow
                        }]
                    }]
                },
                "streamSettings": stream,
                "tag": "proxy-out"
            }))
        }
        "vmess" => {
            let uuid = inner
                .get("uuid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut user = json!({
                "id": uuid,
                "alterId": inner.get("alterId").and_then(|v| v.as_u64()).unwrap_or(0),
                "security": inner.get("security").cloned().unwrap_or(json!("auto")),
            });
            if let Some(a) = inner.get("alter_id").and_then(|v| v.as_u64()) {
                user["alterId"] = json!(a);
            }
            Ok(json!({
                "protocol": "vmess",
                "settings": {
                    "vnext": [{
                        "address": addr,
                        "port": port,
                        "users": [user]
                    }]
                },
                "streamSettings": stream,
                "tag": "proxy-out"
            }))
        }
        "trojan" => {
            let password = inner
                .get("password")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(json!({
                "protocol": "trojan",
                "settings": {
                    "servers": [{
                        "address": addr,
                        "port": port,
                        "password": password
                    }]
                },
                "streamSettings": stream,
                "tag": "proxy-out"
            }))
        }
        "shadowsocks" => {
            let method = inner
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("2022-blake3-aes-128-gcm");
            let password = inner
                .get("password")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({
                "protocol": "shadowsocks",
                "settings": {
                    "servers": [{
                        "address": addr,
                        "port": port,
                        "method": method,
                        "password": password
                    }]
                },
                "streamSettings": stream,
                "tag": "proxy-out"
            }))
        }
        "socks" => {
            let mut u = inner.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let mut p = inner.get("password").and_then(|v| v.as_str()).unwrap_or("");
            if u.is_empty() {
                if let Some(up) = inner.get("users").and_then(|v| v.as_array()).and_then(|a| a.first()) {
                    u = up.get("username").and_then(|v| v.as_str()).unwrap_or("");
                    p = up.get("password").and_then(|v| v.as_str()).unwrap_or("");
                }
            }
            Ok(json!({
                "protocol": "socks",
                "settings": {
                    "servers": [{
                        "address": addr,
                        "port": port,
                        "users": [{
                            "user": u,
                            "pass": p
                        }]
                    }]
                },
                "streamSettings": stream,
                "tag": "proxy-out"
            }))
        }
        "hysteria2" => {
            let password = inner
                .get("password")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({
                "protocol": "hysteria2",
                "settings": {
                    "servers": [{
                        "address": addr,
                        "port": port,
                        "password": password
                    }]
                },
                "streamSettings": stream,
                "tag": "proxy-out"
            }))
        }
        _ => Err(format!("unsupported xray protocol {x}")),
    }
}

/// Build a full Xray JSON document for importing into Xray / v2rayN clients.
pub fn build_xray_client_config(
    row: &GrpcProxySessionRow,
    opts: &XrayExportOptions,
) -> Result<Value, String> {
    if row.revoked {
        return Err("session revoked".into());
    }
    let addr = opts.server_address.trim();
    if addr.is_empty() {
        return Err("server address is empty".into());
    }
    let port = row
        .ingress_listen_port
        .ok_or_else(|| "ingress_listen_port is NULL".to_string())?;
    if port <= 0 || port > 65535 {
        return Err("invalid ingress_listen_port".into());
    }
    let stream = merge_stream_settings(
        row.ingress_tls_json.as_deref(),
        opts.tls_server_name.as_deref(),
    )?;
    let outbound = outbound_from_row(row, addr, port, stream)?;
    Ok(json!({
        "log": { "loglevel": "warning" },
        "inbounds": [{
            "listen": "127.0.0.1",
            "port": 10808,
            "protocol": "socks",
            "settings": { "udp": true },
            "tag": "socks-in"
        }],
        "outbounds": [outbound, {
            "protocol": "freedom",
            "tag": "direct"
        }],
        "routing": {
            "domainStrategy": "AsIs",
            "rules": [{ "type": "field", "outboundTag": "proxy-out", "network": "tcp,udp" }]
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn vless_minimal_snapshot() {
        let row = GrpcProxySessionRow {
            session_id: "sid".into(),
            client_pubkey_b64: "pk".into(),
            board_label: "b".into(),
            token_sha256_hex: "00".into(),
            subscription_token: None,
            created_at: Utc::now(),
            expires_at: Utc::now(),
            policy_json: "{}".into(),
            bytes_in: 0,
            bytes_out: 0,
            active_ms: 0,
            last_activity_at: None,
            first_open_at: None,
            revoked: false,
            wire_mode: None,
            wire_config_json: None,
            ingress_protocol: Some(1),
            ingress_listen_port: Some(443),
            ingress_listen_udp_port: None,
            ingress_config_json: Some(r#"{"uuid":"11111111-1111-1111-1111-111111111111","flow":"xtls-rprx-vision"}"#.into()),
            ingress_tls_json: Some(r#"{"enabled":true,"server_name":"example.com","alpn":["h2"]}"#.into()),
            ingress_template_version: 1,
        };
        let v = build_xray_client_config(
            &row,
            &XrayExportOptions {
                server_address: "ingress.example.com".into(),
                tls_server_name: None,
            },
        )
        .unwrap();
        let out = v["outbounds"][0]["protocol"].as_str().unwrap();
        assert_eq!(out, "vless");
        let addr = v["outbounds"][0]["settings"]["vnext"][0]["address"].as_str().unwrap();
        assert_eq!(addr, "ingress.example.com");
    }
}
