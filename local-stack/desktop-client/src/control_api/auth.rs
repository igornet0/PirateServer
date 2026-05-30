//! control-api JWT login / session.

use crate::connection::{
    clear_control_api_jwt, load_control_api_jwt, save_control_api_jwt,
};
use serde::Deserialize;

use super::rest::{fmt_reqwest_send_err, health_probe_summary, normalize_base, now_ms};

#[derive(Deserialize)]
struct LoginResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

/// POST `/api/v1/auth/login`, store JWT + expiry (`expires_in` seconds).
pub fn control_api_login(base_url: &str, username: &str, password: &str) -> Result<(), String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    let u = username.trim();
    let p = password.trim();
    if u.is_empty() || p.is_empty() {
        return Err("username and password required".into());
    }

    let url = format!("{}/api/v1/auth/login", base);
    let client = crate::http_client::blocking_client()?;

    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "username": u, "password": p }))
        .send()
        .map_err(|e| {
            let mut out = fmt_reqwest_send_err(e, &url);
            out.push_str(&format!(" (base: {base}; {})", health_probe_summary(&base)));
            out
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "login HTTP {}: {}",
            status,
            body.chars().take(200).collect::<String>()
        ));
    }

    let login: LoginResponse = resp.json().map_err(|e| e.to_string())?;
    let token = login.access_token.trim().to_string();
    if token.is_empty() {
        return Err("empty access_token in login response".into());
    }

    let expires_at_ms = if login.expires_in > 0 {
        now_ms().saturating_add((login.expires_in as i64).saturating_mul(1000))
    } else {
        now_ms().saturating_add(86400 * 1000)
    };

    save_control_api_jwt(&token, expires_at_ms)?;
    Ok(())
}

pub fn control_api_health_probe(base_url: &str) -> Result<String, String> {
    let base = normalize_base(base_url);
    if base.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    Ok(health_probe_summary(&base))
}

pub fn control_api_logout() -> Result<(), String> {
    clear_control_api_jwt()
}

pub fn control_api_session_active() -> bool {
    bearer().is_ok()
}

pub fn control_api_bearer_token() -> Result<String, String> {
    bearer()
}

pub(crate) fn bearer() -> Result<String, String> {
    let Some((tok, exp)) = load_control_api_jwt() else {
        return Err("not logged in to control-api".into());
    };
    if tok.is_empty() {
        return Err("not logged in to control-api".into());
    }
    if exp > 0 && now_ms() >= exp - 30_000 {
        return Err("control-api session expired; sign in again".into());
    }
    Ok(tok)
}
