//! HTTP helpers for `stack-tun-api` (REST control-plane).

fn blocking_client() -> Result<&'static reqwest::blocking::Client, String> {
    crate::http_client::stack_tun_client()
}

pub fn normalize_stack_tun_base(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn bearer_auth(
    rb: reqwest::blocking::RequestBuilder,
    bearer: Option<&str>,
) -> reqwest::blocking::RequestBuilder {
    match bearer.map(str::trim).filter(|s| !s.is_empty()) {
        Some(tok) => rb.header(reqwest::header::AUTHORIZATION, format!("Bearer {tok}")),
        None => rb,
    }
}

pub fn stack_tun_health(body: &str, bearer: Option<&str>) -> Result<String, String> {
    let base = normalize_stack_tun_base(body);
    if base.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(format!("{base}/health")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    Ok(format!("{} {}", r.status(), r.text().unwrap_or_default()))
}

pub fn stack_tun_get_config_json(body: &str, bearer: Option<&str>) -> Result<String, String> {
    let base = normalize_stack_tun_base(body);
    if base.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(format!("{base}/api/v1/config")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GET config {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_put_config_json(
    base: &str,
    bearer: Option<&str>,
    json_body: &str,
) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    serde_json::from_str::<serde_json::Value>(json_body.trim())
        .map_err(|e| format!("invalid JSON payload: {e}"))?;

    let c = blocking_client()?;
    let rb = bearer_auth(
        c.put(format!("{b}/api/v1/config"))
            .header("Content-Type", "application/json"),
        bearer,
    );
    let r = rb
        .body(json_body.trim().to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("PUT config {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_stats_json(base: &str, bearer: Option<&str>) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(format!("{b}/api/v1/stats")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("stats {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_reload_peers(base: &str, bearer: Option<&str>) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.post(format!("{b}/api/v1/peers/reload")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("peers reload {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_identity_public_key_json(base: &str, bearer: Option<&str>) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(format!("{b}/api/v1/identity/public-key")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("identity public key {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_list_peers_json(base: &str, bearer: Option<&str>) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(format!("{b}/api/v1/peers")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GET peers {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_get_routes_json(base: &str, bearer: Option<&str>) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(format!("{b}/api/v1/routes")), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GET routes {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_put_routes_json(
    base: &str,
    bearer: Option<&str>,
    json_body: &str,
) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    serde_json::from_str::<serde_json::Value>(json_body.trim())
        .map_err(|e| format!("invalid JSON routes payload: {e}"))?;
    let c = blocking_client()?;
    let rb = bearer_auth(
        c.put(format!("{b}/api/v1/routes"))
            .header("Content-Type", "application/json"),
        bearer,
    );
    let r = rb
        .body(json_body.trim().to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("PUT routes {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_requests_json(
    base: &str,
    bearer: Option<&str>,
    query: Option<&str>,
) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let suffix = match query.map(str::trim).filter(|x| !x.is_empty()) {
        None => String::new(),
        Some(q) if q.starts_with('?') => q.to_string(),
        Some(q) => format!("?{}", q.trim_start_matches('?')),
    };
    let path = format!("{b}/api/v1/requests{}", suffix);
    let c = blocking_client()?;
    let rb = bearer_auth(c.get(path), bearer);
    let r = rb.send().map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GET requests {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_request_bus_invoke_json(
    base: &str,
    bearer: Option<&str>,
    json_body: &str,
) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    serde_json::from_str::<serde_json::Value>(json_body.trim())
        .map_err(|e| format!("invalid JSON invoke payload: {e}"))?;
    let c = blocking_client()?;
    let rb = bearer_auth(
        c.post(format!("{b}/api/v1/request-bus/invoke"))
            .header("Content-Type", "application/json"),
        bearer,
    );
    let r = rb
        .body(json_body.trim().to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("request-bus invoke {status}: {txt}"));
    }
    Ok(txt)
}

pub fn stack_tun_authorize_peer_json(
    base: &str,
    bearer: Option<&str>,
    public_key_b64: &str,
) -> Result<String, String> {
    let b = normalize_stack_tun_base(base);
    if b.is_empty() {
        return Err("stack-tun base URL is empty".into());
    }
    let body = serde_json::json!({ "publicKeyB64": public_key_b64.trim() });
    let c = blocking_client()?;
    let rb = bearer_auth(
        c.post(format!("{b}/api/v1/peers"))
            .header("Content-Type", "application/json"),
        bearer,
    );
    let r = rb
        .body(body.to_string())
        .send()
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let txt = r.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("authorize peer {status}: {txt}"));
    }
    Ok(txt)
}
