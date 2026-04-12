//! JSON `wire_config_json` schema shared by client and server.

use serde::{Deserialize, Serialize};

/// Mirrors [`deploy_proto::deploy::ProxyWireMode`] (keep in sync manually).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireMode {
    RawTcpRelay,
    Vless,
    Trojan,
    Vmess,
}

impl Default for WireMode {
    fn default() -> Self {
        Self::RawTcpRelay
    }
}

impl WireMode {
    pub fn from_proto(n: i32) -> Self {
        match n {
            1 => Self::Vless,
            2 => Self::Trojan,
            3 => Self::Vmess,
            _ => Self::RawTcpRelay,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            WireMode::RawTcpRelay => 0,
            WireMode::Vless => 1,
            WireMode::Trojan => 2,
            WireMode::Vmess => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireParams {
    /// VLESS / VMess user id (UUID string).
    #[serde(default)]
    pub uuid: Option<String>,
    /// Trojan password (plaintext; hashed on the wire).
    #[serde(default)]
    pub password: Option<String>,
    /// Optional flow label (reserved).
    #[serde(default)]
    pub flow: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("invalid wire config json: {0}")]
    BadConfig(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("protocol: {0}")]
    Protocol(String),
}

impl WireParams {
    pub fn from_json(s: &str) -> Result<Self, WireError> {
        if s.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(s).map_err(|e| WireError::BadConfig(e.to_string()))
    }
}
