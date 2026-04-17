//! Shared JSON config for desktop display streaming (producer → HTTP ingest → consumer).
//!
//! Export from dashboard as `data:application/json;base64,...` or raw JSON.

use serde::{Deserialize, Serialize};

pub const DISPLAY_STREAM_CONFIG_V: u32 = 1;

/// `data:application/json;base64,` prefix for clipboard / deep links.
pub const DATA_URL_PREFIX: &str = "data:application/json;base64,";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStreamRole {
    Producer,
    Consumer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStreamEncrypt {
    #[default]
    None,
    Tls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStreamProtocol {
    #[default]
    HttpPostJpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayStreamConfig {
    pub v: u32,
    pub role: DisplayStreamRole,
    /// Producer: POST target base (e.g. `http://192.168.1.10:39100/ingest`). Consumer: local bind is chosen at runtime; this is the URL to show (e.g. `http://127.0.0.1:39100/ingest`).
    pub ingest_base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    #[serde(default = "default_quality")]
    pub quality: u8,
    #[serde(default = "default_fps")]
    pub fps: u8,
    #[serde(default)]
    pub display_index: u32,
    #[serde(default)]
    pub protocol: DisplayStreamProtocol,
    #[serde(default = "default_encrypt")]
    pub encrypt: DisplayStreamEncrypt,
}

fn default_quality() -> u8 {
    70
}

fn default_fps() -> u8 {
    10
}

fn default_encrypt() -> DisplayStreamEncrypt {
    DisplayStreamEncrypt::None
}

impl DisplayStreamConfig {
    pub fn example_producer(ingest_url: &str) -> Self {
        Self {
            v: DISPLAY_STREAM_CONFIG_V,
            role: DisplayStreamRole::Producer,
            ingest_base_url: ingest_url.to_string(),
            token: String::new(),
            quality: 70,
            fps: 10,
            display_index: 0,
            protocol: DisplayStreamProtocol::HttpPostJpeg,
            encrypt: DisplayStreamEncrypt::None,
        }
    }

    pub fn example_consumer(local_ingest: &str) -> Self {
        Self {
            v: DISPLAY_STREAM_CONFIG_V,
            role: DisplayStreamRole::Consumer,
            ingest_base_url: local_ingest.to_string(),
            token: String::new(),
            quality: 70,
            fps: 10,
            display_index: 0,
            protocol: DisplayStreamProtocol::HttpPostJpeg,
            encrypt: DisplayStreamEncrypt::None,
        }
    }

    /// Normalize and validate after JSON parse.
    pub fn validate(&self) -> Result<(), String> {
        if self.v != DISPLAY_STREAM_CONFIG_V {
            return Err(format!("unsupported config v: {}", self.v));
        }
        let u = self.ingest_base_url.trim();
        if u.is_empty() {
            return Err("ingest_base_url is empty".into());
        }
        if !u.starts_with("http://") && !u.starts_with("https://") {
            return Err("ingest_base_url must start with http:// or https://".into());
        }
        if self.quality == 0 || self.quality > 100 {
            return Err("quality must be 1–100".into());
        }
        if self.fps == 0 || self.fps > 60 {
            return Err("fps must be 1–60".into());
        }
        Ok(())
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// `data:application/json;base64,<standard base64 JSON>`
    pub fn to_data_url(&self) -> Result<String, serde_json::Error> {
        let json = self.to_json_string()?;
        let b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, json.as_bytes());
        Ok(format!("{DATA_URL_PREFIX}{b64}"))
    }

    /// Parse `data:application/json;base64,...` or raw JSON object string.
    pub fn from_data_url_or_json(s: &str) -> Result<Self, String> {
        let t = s.trim();
        if let Some(rest) = t.strip_prefix(DATA_URL_PREFIX) {
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, rest.trim())
                    .map_err(|e| e.to_string())?;
            let json = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            let c: Self = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            c.validate()?;
            return Ok(c);
        }
        let c: Self = serde_json::from_str(t).map_err(|e| e.to_string())?;
        c.validate()?;
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_data_url() {
        let c = DisplayStreamConfig::example_producer("http://127.0.0.1:39100/ingest");
        let url = c.to_data_url().unwrap();
        assert!(url.starts_with(DATA_URL_PREFIX));
        let c2 = DisplayStreamConfig::from_data_url_or_json(&url).unwrap();
        assert_eq!(c, c2);
    }

    #[test]
    fn raw_json() {
        let j = r#"{"v":1,"role":"producer","ingest_base_url":"http://x/ingest","token":"","quality":50,"fps":5,"display_index":1,"protocol":"http_post_jpeg","encrypt":"tls"}"#;
        let c = DisplayStreamConfig::from_data_url_or_json(j).unwrap();
        assert_eq!(c.display_index, 1);
        assert_eq!(c.encrypt, DisplayStreamEncrypt::Tls);
    }
}
