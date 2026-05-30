//! Direct connection profile passwords: encrypted entries in a local JSON file (no OS keychain).
//! Uses the same `host_db_credentials.key` + AES-256-GCM as `db_credentials` (see `credentials_crypto`).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;

use fs4::fs_std::FileExt;
use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::credentials_crypto::{decrypt_password, encrypt_password, EncryptedEnvelope};
use crate::db_credentials::{desktop_local_aes_key_path, desktop_pirate_data_root};

const FILE_VERSION: u32 = 1;
const LEGACY_KEYRING_SERVICE: &str = "PirateClient.db_direct_password";

const DIRECT_PASSWORDS_JSON: &str = "db_direct_passwords.json";
const DIRECT_PASSWORDS_LOCK: &str = "db_direct_passwords.lock";

fn json_path() -> PathBuf {
    desktop_pirate_data_root().join(DIRECT_PASSWORDS_JSON)
}

fn lock_path() -> PathBuf {
    desktop_pirate_data_root().join(DIRECT_PASSWORDS_LOCK)
}

fn key_path() -> PathBuf {
    desktop_local_aes_key_path()
}

fn ensure_data_root() -> Result<(), String> {
    let root = desktop_pirate_data_root();
    std::fs::create_dir_all(&root).map_err(|e| format!("create PirateClient data dir: {e}"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DirectPasswordFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    profiles: HashMap<String, DirectPasswordRecord>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct DirectPasswordRecord {
    #[serde(default)]
    enc: Option<EncryptedEnvelope>,
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

/// Legacy: read password from macOS Keychain / Windows Credential Manager (Pirate < current).
fn keyring_get_password(profile_id: &str) -> Result<Option<String>, String> {
    let e = Entry::new(LEGACY_KEYRING_SERVICE, profile_id).map_err(|e| e.to_string())?;
    match e.get_password() {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => Ok(Some(s)),
        Err(err) if is_no_such_keyring(&err) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn keyring_delete_password(profile_id: &str) {
    if let Ok(e) = Entry::new(LEGACY_KEYRING_SERVICE, profile_id) {
        let _ = e.delete_password();
    }
}

#[cfg(unix)]
fn restrict_json_perms(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn restrict_json_perms(_path: &std::path::Path) {}

fn load_file() -> Result<DirectPasswordFile, String> {
    let path = json_path();
    if !path.is_file() {
        return Ok(DirectPasswordFile {
            version: FILE_VERSION,
            ..Default::default()
        });
    }
    let mut s = String::new();
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_to_string(&mut s))
        .map_err(|e| format!("read {DIRECT_PASSWORDS_JSON}: {e}"))?;
    if s.trim().is_empty() {
        return Ok(DirectPasswordFile {
            version: FILE_VERSION,
            ..Default::default()
        });
    }
    serde_json::from_str(&s).map_err(|e| {
        format!(
            "corrupt {DIRECT_PASSWORDS_JSON} ({e}); path: {}",
            path.display()
        )
    })
}

fn atomic_write_file(doc: &DirectPasswordFile) -> Result<(), String> {
    let path = json_path();
    let parent = path
        .parent()
        .ok_or_else(|| "direct password json path has no parent".to_string())?;
    let _ = std::fs::create_dir_all(parent);
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(doc).map_err(|e| format!("serialize: {e}"))?;
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("write temp: {e}"))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("write temp: {e}"))?;
        f.sync_all().map_err(|e| format!("sync temp: {e}"))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("commit {DIRECT_PASSWORDS_JSON}: {e}"))?;
    restrict_json_perms(&path);
    Ok(())
}

fn with_locked_store<R, F>(f: F) -> Result<R, String>
where
    F: FnOnce(&mut DirectPasswordFile) -> Result<R, String>,
{
    ensure_data_root()?;
    let lock_f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(lock_path())
        .map_err(|e| format!("open direct password lock: {e}"))?;
    lock_f
        .lock_exclusive()
        .map_err(|e| format!("lock direct password store: {e}"))?;
    let mut doc = load_file()?;
    if doc.version == 0 {
        doc.version = FILE_VERSION;
    }
    f(&mut doc)
}

fn read_decrypted_from_file(profile_id: &str) -> Result<Option<String>, String> {
    let id = profile_id.to_string();
    with_locked_store(|doc| {
        if let Some(rec) = doc.profiles.get(&id) {
            if let Some(enc) = &rec.enc {
                let plain = decrypt_password(&key_path(), enc)?;
                return Ok(Some(plain));
            }
        }
        Ok(None)
    })
}

/// Decrypt password for this profile, migrating from the legacy keychain into JSON (encrypted) once if present.
pub fn get(profile_id: &str) -> Result<Option<String>, String> {
    if let Some(p) = read_decrypted_from_file(profile_id)? {
        return Ok(Some(p));
    }
    if let Some(plain) = keyring_get_password(profile_id)? {
        set(profile_id, &plain)?;
        return Ok(Some(plain));
    }
    Ok(None)
}

/// Save or clear encrypted password. Does not use the keychain; clears a legacy keychain entry if any.
pub fn set(profile_id: &str, password: &str) -> Result<(), String> {
    let id = profile_id.to_string();
    with_locked_store(|doc| {
        if password.is_empty() {
            doc.profiles.remove(&id);
        } else {
            let enc = encrypt_password(&key_path(), password)?;
            doc.profiles
                .insert(id, DirectPasswordRecord { enc: Some(enc) });
        }
        atomic_write_file(doc)
    })?;
    keyring_delete_password(profile_id);
    Ok(())
}

/// Whether an encrypted password exists in `db_direct_passwords.json` (does not touch the legacy keychain —
/// avoids OS keychain prompts when listing profiles; migration runs on `get`).
pub fn has(profile_id: &str) -> Result<bool, String> {
    let id = profile_id.to_string();
    with_locked_store(|doc| {
        Ok(doc
            .profiles
            .get(&id)
            .map(|r| r.enc.is_some())
            .unwrap_or(false))
    })
}
