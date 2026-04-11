//! Paths and JSON for client identity and server connection (shared layout with desktop).

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("pirate-client"))
}

pub fn identity_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("identity.json"))
}

pub fn connection_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("connection.json"))
}

pub fn load_or_create_identity() -> Result<SigningKey, Box<dyn std::error::Error>> {
    let path = identity_path().ok_or("no config directory")?;
    deploy_auth::load_or_create_identity(&path).map_err(|e| e.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConnection {
    pub url: String,
    pub server_pubkey_b64: String,
    #[serde(default)]
    pub paired: bool,
}

pub fn load_connection() -> Option<StoredConnection> {
    let path = connection_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_connection(c: &StoredConnection) -> Result<(), Box<dyn std::error::Error>> {
    let dir = config_dir().ok_or("no config directory")?;
    std::fs::create_dir_all(&dir)?;
    let path = connection_path().ok_or("no config directory")?;
    std::fs::write(path, serde_json::to_string_pretty(c)?)?;
    Ok(())
}
