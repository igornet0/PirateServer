//! Per-instance DB viewer credentials: encrypted password + username in `host_db_credentials.json`.
//! Direct (non-tunnel) DB profile passwords use `db_direct_passwords.json` (same AES key file).
//! Optional override: `PIRATE_DESKTOP_DATA_DIR` (tests / advanced); otherwise `dirs::data_local_dir()/PirateClient`.
//! Server receives credentials only in request headers (not persisted server-side).

use crate::credentials_crypto::{decrypt_password, encrypt_password, EncryptedEnvelope};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const FILE_VERSION: u32 = 1;

/// Env var: if set to a non-empty path, all host DB credential files are stored under this directory
/// (instead of the default `PirateClient` data folder).
pub const DATA_DIR_ENV: &str = "PIRATE_DESKTOP_DATA_DIR";

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn pirate_data_root() -> PathBuf {
    if let Ok(p) = std::env::var(DATA_DIR_ENV) {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
}

/// `~/Library/Application Support/.../PirateClient` (or `PIRATE_DESKTOP_DATA_DIR`) — shared with direct DB profile storage.
pub fn desktop_pirate_data_root() -> PathBuf {
    pirate_data_root()
}

/// Same AES-256 key file as host DB credentials (`host_db_credentials.key`); used for all local encrypted JSON stores.
pub fn desktop_local_aes_key_path() -> PathBuf {
    cred_paths().key
}

struct CredPaths {
    root: PathBuf,
    json: PathBuf,
    key: PathBuf,
    lock: PathBuf,
}

fn cred_paths() -> CredPaths {
    let root = pirate_data_root();
    CredPaths {
        json: root.join("host_db_credentials.json"),
        key: root.join("host_db_credentials.key"),
        lock: root.join("host_db_credentials.lock"),
        root,
    }
}

fn ensure_root(root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("create credential directory: {e}"))
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    instances: HashMap<String, InstanceRecord>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct InstanceRecord {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    remember: bool,
    #[serde(default)]
    enc: Option<EncryptedEnvelope>,
    #[serde(default)]
    updated_at_ms: i64,
}

fn load_file(json_path: &Path) -> Result<CredentialFile, String> {
    if !json_path.is_file() {
        return Ok(CredentialFile {
            version: FILE_VERSION,
            instances: HashMap::new(),
        });
    }
    let mut s = String::new();
    std::fs::File::open(json_path)
        .and_then(|mut f| f.read_to_string(&mut s))
        .map_err(|e| format!("read host DB credentials file: {e}"))?;
    if s.trim().is_empty() {
        return Ok(CredentialFile {
            version: FILE_VERSION,
            instances: HashMap::new(),
        });
    }
    serde_json::from_str(&s).map_err(|e| {
        format!(
            "host DB credentials file is corrupt or unreadable: {e} (path: {})",
            json_path.display()
        )
    })
}

fn atomic_write_json(path: &Path, doc: &CredentialFile) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "credentials path has no parent directory".to_string())?;
    let _ = std::fs::create_dir_all(parent);
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_string_pretty(doc)
        .map_err(|e| format!("serialize credentials: {e}"))?;
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("write temp credentials: {e}"))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("write temp credentials: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("sync temp credentials: {e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("commit credentials file: {e}"))?;
    #[cfg(unix)]
    restrict_json_permissions(path);
    Ok(())
}

#[cfg(unix)]
fn restrict_json_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn restrict_json_permissions(_path: &Path) {}

fn with_locked_file<R, F>(paths: &CredPaths, f: F) -> Result<R, String>
where
    F: FnOnce(&mut CredentialFile) -> Result<R, String>,
{
    ensure_root(&paths.root)?;
    let lock_f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&paths.lock)
        .map_err(|e| format!("open credentials lock: {e}"))?;
    lock_f
        .lock_exclusive()
        .map_err(|e| format!("lock host DB credentials: {e}"))?;
    let mut doc = load_file(&paths.json)?;
    if doc.version == 0 {
        doc.version = FILE_VERSION;
    }
    let out = f(&mut doc)?;
    Ok(out)
}

fn persist_after_mutate(paths: &CredPaths, doc: &CredentialFile) -> Result<(), String> {
    atomic_write_json(&paths.json, doc)
}

/// Returns decrypted saved password for this host DB instance, if any (for control-api HTTP only).
pub(crate) fn saved_password_plain(instance_id: &str) -> Result<Option<String>, String> {
    let paths = cred_paths();
    with_locked_file(&paths, |doc| {
        let Some(rec) = doc.instances.get(instance_id) else {
            return Ok(None);
        };
        let Some(enc) = &rec.enc else {
            return Ok(None);
        };
        let p = decrypt_password(&paths.key, enc)?;
        Ok(Some(p))
    })
}

/// JSON for UI: `username`, `remember`, `hasSavedPassword`.
pub fn db_credentials_get_json(instance_id: &str) -> Result<String, String> {
    let paths = cred_paths();
    let (username, remember, has_saved) = with_locked_file(&paths, |doc| {
        let rec = doc.instances.get(instance_id);
        let username = rec.and_then(|r| r.username.clone());
        let remember = rec.map(|r| r.remember).unwrap_or(false);
        let has_saved = rec.map(|r| r.enc.is_some()).unwrap_or(false);
        Ok((username, remember, has_saved))
    })?;
    let v = serde_json::json!({
        "username": username,
        "remember": remember,
        "hasSavedPassword": has_saved,
    });
    Ok(v.to_string())
}

/// Persists username and optional encrypted password in the JSON store.
/// If `remember` is true and password is empty, keeps existing ciphertext when present.
pub fn db_credentials_save(
    instance_id: &str,
    username: &str,
    password: &str,
    remember: bool,
) -> Result<(), String> {
    let paths = cred_paths();
    let id = instance_id.to_string();
    with_locked_file(&paths, |doc| {
        if remember {
            let enc = if password.is_empty() {
                let prev = doc.instances.get(instance_id).and_then(|r| r.enc.clone());
                if prev.is_none() {
                    return Err(
                        "enter a database password, or use Forget to clear saved credentials".into(),
                    );
                }
                prev
            } else {
                Some(encrypt_password(&paths.key, password)?)
            };
            doc.instances.insert(
                id.clone(),
                InstanceRecord {
                    username: if username.trim().is_empty() {
                        None
                    } else {
                        Some(username.trim().to_string())
                    },
                    remember: true,
                    enc,
                    updated_at_ms: now_ms(),
                },
            );
        } else {
            doc.instances.insert(
                id.clone(),
                InstanceRecord {
                    username: if username.trim().is_empty() {
                        None
                    } else {
                        Some(username.trim().to_string())
                    },
                    remember: false,
                    enc: None,
                    updated_at_ms: now_ms(),
                },
            );
        }
        doc.version = FILE_VERSION;
        persist_after_mutate(&paths, doc)?;
        Ok(())
    })?;
    Ok(())
}

/// Removes stored credentials for this instance.
pub fn db_credentials_forget(instance_id: &str) -> Result<(), String> {
    let paths = cred_paths();
    let id = instance_id.to_string();
    with_locked_file(&paths, |doc| {
        doc.instances.remove(&id);
        doc.version = FILE_VERSION;
        persist_after_mutate(&paths, doc)?;
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::tempdir;

    fn set_dir(d: &Path) {
        std::env::set_var(DATA_DIR_ENV, d.to_str().unwrap());
    }

    fn clear_dir() {
        std::env::remove_var(DATA_DIR_ENV);
    }

    #[test]
    #[serial]
    fn save_get_forget_roundtrip() {
        let dir = tempdir().unwrap();
        set_dir(dir.path());
        let id = "inst-1";
        db_credentials_save(id, "alice", "secret", true).unwrap();
        let j = db_credentials_get_json(id).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["username"], "alice");
        assert_eq!(v["remember"], true);
        assert_eq!(v["hasSavedPassword"], true);
        let p = saved_password_plain(id).unwrap();
        assert_eq!(p.as_deref(), Some("secret"));
        db_credentials_forget(id).unwrap();
        let j2 = db_credentials_get_json(id).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&j2).unwrap();
        assert!(v2["username"].is_null());
        assert_eq!(v2["hasSavedPassword"], false);
        clear_dir();
    }

    #[test]
    #[serial]
    fn remember_without_new_password_keeps_cipher() {
        let dir = tempdir().unwrap();
        set_dir(dir.path());
        let id = "inst-2";
        db_credentials_save(id, "bob", "first", true).unwrap();
        db_credentials_save(id, "bob", "", true).unwrap();
        assert_eq!(saved_password_plain(id).unwrap().as_deref(), Some("first"));
        clear_dir();
    }

    #[test]
    #[serial]
    fn remember_off_clears_enc() {
        let dir = tempdir().unwrap();
        set_dir(dir.path());
        let id = "inst-3";
        db_credentials_save(id, "u", "p", true).unwrap();
        db_credentials_save(id, "u", "", false).unwrap();
        assert!(saved_password_plain(id).unwrap().is_none());
        let j = db_credentials_get_json(id).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["hasSavedPassword"], false);
        clear_dir();
    }

    #[test]
    #[serial]
    fn corrupt_credentials_file_returns_error() {
        let dir = tempdir().unwrap();
        set_dir(dir.path());
        let _ = std::fs::create_dir_all(dir.path());
        let p = dir.path().join("host_db_credentials.json");
        std::fs::write(&p, "{ not valid json").unwrap();
        let err = db_credentials_get_json("any").unwrap_err();
        assert!(
            err.contains("corrupt") || err.contains("unreadable"),
            "unexpected error: {err}"
        );
        clear_dir();
    }
}
