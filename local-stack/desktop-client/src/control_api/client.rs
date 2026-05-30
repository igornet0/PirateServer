//! Typed control-api HTTP helper (shared connection pool).

use reqwest::blocking::Response;
use reqwest::StatusCode;

use super::auth;
use super::rest::{format_control_api_http_error, normalize_base};

/// Control-api session for a single base URL.
pub struct ControlApiClient {
    pub base: String,
}

impl ControlApiClient {
    pub fn new(base_url: &str) -> Result<Self, String> {
        let base = normalize_base(base_url);
        if base.is_empty() {
            return Err("control-api base URL is empty".into());
        }
        Ok(Self { base })
    }

    pub fn bearer(&self) -> Result<String, String> {
        auth::bearer()
    }

    pub fn get(&self, path: &str) -> Result<Response, String> {
        let token = self.bearer()?;
        let url = format!("{}{}", self.base, path);
        crate::http_client::blocking_client()?
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .map_err(|e| e.to_string())
    }

    pub fn get_text(&self, path: &str, action: &str) -> Result<String, String> {
        let resp = self.get(path)?;
        let status = resp.status();
        let body = resp.text().map_err(|e| e.to_string())?;
        if !status.is_success() {
            if status == StatusCode::UNAUTHORIZED {
                let _ = crate::connection::clear_control_api_jwt();
            }
            return Err(format_control_api_http_error(status, &body, action));
        }
        Ok(body)
    }
}
