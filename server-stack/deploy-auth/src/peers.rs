//! Persisted list of authorized client Ed25519 public keys (URL-safe Base64).

use super::{parse_verifying_key_b64, AuthError};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64U;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AuthorizedPeers {
    pub peers: Vec<String>,
}

pub fn load_authorized_peers(path: &Path) -> Result<HashSet<[u8; 32]>, AuthError> {
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let s = std::fs::read_to_string(path)?;
    let file: AuthorizedPeers = serde_json::from_str(&s).unwrap_or_default();
    let mut out = HashSet::new();
    for p in file.peers {
        let vk = parse_verifying_key_b64(&p)?;
        out.insert(*vk.as_bytes());
    }
    Ok(out)
}

pub fn save_authorized_peers(path: &Path, keys: &HashSet<[u8; 32]>) -> Result<(), AuthError> {
    let mut peers: Vec<String> = keys
        .iter()
        .map(|b| B64U.encode(b))
        .collect();
    peers.sort();
    let file = AuthorizedPeers { peers };
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&file)?)?;
    Ok(())
}
