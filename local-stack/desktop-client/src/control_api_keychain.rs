//! control-api username/password in the OS credential store (macOS Keychain, Windows Credential Manager, etc.).

use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::control_api::normalize_base;

const KEYRING_SERVICE: &str = "PirateClient.control_api";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlApiKeychainCreds {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
struct StoredPayload {
    username: String,
    password: String,
}

fn is_no_such_keyring(e: &keyring::Error) -> bool {
    match e {
        keyring::Error::NoEntry => true,
        _ => {
            e.to_string().to_lowercase().contains("not found")
                || e.to_string().to_lowercase().contains("no such")
        }
    }
}

fn entry_for_base(base: &str) -> Result<Entry, String> {
    let account = normalize_base(base);
    if account.is_empty() {
        return Err("control-api base URL is empty".into());
    }
    Entry::new(KEYRING_SERVICE, &account).map_err(|e| e.to_string())
}

/// Store credentials for this control-api base URL (replaces any previous entry for the same base).
pub fn control_api_keychain_save(
    base_url: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let u = username.trim();
    let p = password.trim();
    if u.is_empty() || p.is_empty() {
        return Err("username and password required".into());
    }
    let payload = StoredPayload {
        username: u.to_string(),
        password: p.to_string(),
    };
    let json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let e = entry_for_base(base_url)?;
    e.set_password(&json).map_err(|err| err.to_string())
}

/// Load stored credentials for this base URL, if any.
pub fn control_api_keychain_load(
    base_url: &str,
) -> Result<Option<ControlApiKeychainCreds>, String> {
    let e = match entry_for_base(base_url) {
        Ok(e) => e,
        Err(msg) if msg == "control-api base URL is empty" => return Ok(None),
        Err(e) => return Err(e),
    };
    match e.get_password() {
        Ok(s) if s.trim().is_empty() => Ok(None),
        Ok(s) => {
            let p: StoredPayload = serde_json::from_str(&s).map_err(|e| {
                format!(
                    "invalid control-api keychain payload: {e}: {}",
                    s.chars().take(120).collect::<String>()
                )
            })?;
            Ok(Some(ControlApiKeychainCreds {
                username: p.username,
                password: p.password,
            }))
        }
        Err(err) if is_no_such_keyring(&err) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Remove stored credentials for this base URL. No-op if none.
pub fn control_api_keychain_delete(base_url: &str) -> Result<(), String> {
    let e = match entry_for_base(base_url) {
        Ok(e) => e,
        Err(msg) if msg == "control-api base URL is empty" => return Ok(()),
        Err(e) => return Err(e),
    };
    match e.delete_password() {
        Ok(()) => Ok(()),
        Err(err) if is_no_such_keyring(&err) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
