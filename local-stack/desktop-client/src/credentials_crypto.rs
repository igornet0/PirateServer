//! Local symmetric encryption for host DB and direct-profile passwords in desktop JSON files.
//! Uses AES-256-GCM with a random 32-byte key in `host_db_credentials.key` under the Pirate data dir
//! (also used for `db_direct_passwords.json`).

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng as AesOsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Wire format for one ciphertext (evolvable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedEnvelope {
    pub v: u32,
    pub algo: String,
    /// Random nonce, base64 (12 bytes for GCM).
    pub n: String,
    /// Ciphertext (includes auth tag), base64.
    pub c: String,
}

#[derive(Debug)]
pub enum CryptoError {
    UnsupportedVersion(u32),
    UnknownAlgorithm(String),
    InvalidBase64,
    InvalidNonceLen,
    Decrypt,
    Encrypt,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::UnsupportedVersion(v) => write!(f, "unsupported envelope version {v}"),
            CryptoError::UnknownAlgorithm(a) => write!(f, "unknown algorithm {a}"),
            CryptoError::InvalidBase64 => write!(f, "invalid base64 in envelope"),
            CryptoError::InvalidNonceLen => write!(f, "invalid nonce length (expected {NONCE_LEN})"),
            CryptoError::Decrypt => write!(f, "AES-GCM decrypt failed (wrong key or corrupt data)"),
            CryptoError::Encrypt => write!(f, "AES-GCM encrypt failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

fn load_or_create_key(key_path: &Path) -> Result<[u8; KEY_LEN], String> {
    if let Some(parent) = key_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if key_path.is_file() {
        let raw = std::fs::read(key_path).map_err(|e| format!("read credential key: {e}"))?;
        if raw.len() != KEY_LEN {
            return Err(format!(
                "credential key file has wrong length ({} bytes, expected {KEY_LEN})",
                raw.len()
            ));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&raw);
        return Ok(key);
    }
    let mut key = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut key);
    atomic_write_bytes(key_path, &key)?;
    #[cfg(unix)]
    restrict_key_file_permissions(key_path);
    Ok(key)
}

#[cfg(unix)]
fn restrict_key_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn restrict_key_file_permissions(_path: &Path) {}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "credential key path has no parent".to_string())?;
    let _ = std::fs::create_dir_all(parent);
    let tmp = path.with_extension("key.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("write temp credential key: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename credential key: {e}"))?;
    Ok(())
}

/// Encrypt `plaintext` using the key file at `key_path` (created if missing).
pub fn encrypt_password(key_path: &Path, plaintext: &str) -> Result<EncryptedEnvelope, String> {
    let key = load_or_create_key(key_path)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("AES key: {e}"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut AesOsRng);
    let ct = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| CryptoError::Encrypt.to_string())?;
    Ok(EncryptedEnvelope {
        v: 1,
        algo: "aes-256-gcm".into(),
        n: B64.encode(nonce),
        c: B64.encode(ct),
    })
}

/// Decrypt envelope using the key file at `key_path`.
pub fn decrypt_password(key_path: &Path, env: &EncryptedEnvelope) -> Result<String, String> {
    if env.v != 1 {
        return Err(CryptoError::UnsupportedVersion(env.v).to_string());
    }
    if env.algo != "aes-256-gcm" {
        return Err(CryptoError::UnknownAlgorithm(env.algo.clone()).to_string());
    }
    let key = load_or_create_key(key_path)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("AES key: {e}"))?;
    let nonce_bytes = B64
        .decode(env.n.as_bytes())
        .map_err(|_| CryptoError::InvalidBase64.to_string())?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(CryptoError::InvalidNonceLen.to_string());
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = B64
        .decode(env.c.as_bytes())
        .map_err(|_| CryptoError::InvalidBase64.to_string())?;
    let plain = cipher
        .decrypt(nonce, ct.as_ref())
        .map_err(|_| CryptoError::Decrypt.to_string())?;
    String::from_utf8(plain).map_err(|e| format!("decrypted password is not valid UTF-8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip() {
        let dir = tempdir().unwrap();
        let key_path = dir.path().join("k.key");
        let env = encrypt_password(&key_path, "secret-db-pass").unwrap();
        let out = decrypt_password(&key_path, &env).unwrap();
        assert_eq!(out, "secret-db-pass");
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let d1 = tempdir().unwrap();
        let d2 = tempdir().unwrap();
        let k1 = d1.path().join("k.key");
        let k2 = d2.path().join("k.key");
        let env = encrypt_password(&k1, "x").unwrap();
        assert!(decrypt_password(&k2, &env).is_err());
    }
}
