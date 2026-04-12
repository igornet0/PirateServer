//! Ed25519 identities, pairing messages, and per-RPC request signing for gRPC metadata.

/// Cargo package version of this crate (linked into `pirate` / deploy clients).
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

mod nonce;
mod peers;

pub use nonce::NonceTracker;
pub use peers::{load_authorized_peers, save_authorized_peers, AuthorizedPeers};

use base64::engine::general_purpose::STANDARD as B64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64U;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;
use tonic::metadata::{Ascii, MetadataMap, MetadataValue};

const PAIR_CLIENT_MSG: &str = "v1|PAIR";
const PAIR_SERVER_MSG: &str = "v1|PAIR_RESP";

/// gRPC metadata keys (lowercase ASCII).
pub const META_PUBKEY: &str = "x-deploy-pubkey";
pub const META_TS: &str = "x-deploy-ts";
pub const META_NONCE: &str = "x-deploy-nonce";
pub const META_SIG: &str = "x-deploy-sig";
/// Must match the first chunk's `version` for Upload streams.
pub const META_VERSION: &str = "x-deploy-version";
/// Optional; when set, must match first chunk `project_id` (omit for `default`).
pub const META_PROJECT: &str = "x-deploy-project";

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid base64: {0}")]
    InvalidBase64(String),
    #[error("invalid key length")]
    InvalidKeyLength,
    #[error("signature verification failed")]
    BadSignature,
    #[error("unknown client public key")]
    UnknownPeer,
    #[error("missing metadata: {0}")]
    MissingMetadata(&'static str),
    #[error("invalid metadata: {0}")]
    InvalidMetadata(String),
    #[error("timestamp skew too large")]
    TimestampSkew,
    #[error("replay: nonce reused")]
    ReplayNonce,
    #[error("pairing code invalid or expired")]
    PairingCode,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Serialized server or client identity (32-byte seed for Ed25519).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityFile {
    /// Standard Base64 of 32-byte secret seed.
    pub private_key_b64: String,
}

impl IdentityFile {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let bytes = signing_key.to_bytes();
        Self {
            private_key_b64: B64.encode(bytes),
        }
    }

    pub fn from_signing_key(sk: &SigningKey) -> Self {
        Self {
            private_key_b64: B64.encode(sk.to_bytes()),
        }
    }

    pub fn to_signing_key(&self) -> Result<SigningKey, AuthError> {
        let raw = B64
            .decode(self.private_key_b64.trim())
            .map_err(|e| AuthError::InvalidBase64(e.to_string()))?;
        if raw.len() != 32 {
            return Err(AuthError::InvalidKeyLength);
        }
        let arr: [u8; 32] = raw
            .try_into()
            .map_err(|_| AuthError::InvalidKeyLength)?;
        Ok(SigningKey::from_bytes(&arr))
    }
}

pub fn load_or_create_identity(path: &Path) -> Result<SigningKey, AuthError> {
    if path.exists() {
        load_identity(path)
    } else {
        let id = IdentityFile::generate();
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&id)?)?;
        id.to_signing_key()
    }
}

/// Read identity from disk only (no create).
pub fn load_identity(path: &Path) -> Result<SigningKey, AuthError> {
    let s = std::fs::read_to_string(path)?;
    let id: IdentityFile = serde_json::from_str(&s)?;
    id.to_signing_key()
}

pub fn pubkey_b64_url(sk: &SigningKey) -> String {
    B64U.encode(sk.verifying_key().as_bytes())
}

pub fn parse_verifying_key_b64(b64: &str) -> Result<VerifyingKey, AuthError> {
    let bytes = B64U
        .decode(b64.trim())
        .map_err(|e| AuthError::InvalidBase64(e.to_string()))?;
    if bytes.len() != 32 {
        return Err(AuthError::InvalidKeyLength);
    }
    let arr: [u8; 32] = bytes.try_into().map_err(|_| AuthError::InvalidKeyLength)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| AuthError::InvalidKeyLength)
}

/// Canonical message for authenticated RPCs (not Pair).
pub fn rpc_canonical(
    method: &str,
    ts_ms: i64,
    nonce: &str,
    payload: &str,
) -> Vec<u8> {
    format!("v1|{method}|{ts_ms}|{nonce}|{payload}").into_bytes()
}

pub fn pair_request_canonical(
    client_pubkey_b64: &str,
    server_pubkey_b64: &str,
    ts_ms: i64,
    nonce: &str,
    pairing_code: &str,
) -> Vec<u8> {
    format!(
        "{PAIR_CLIENT_MSG}|{client_pubkey_b64}|{server_pubkey_b64}|{ts_ms}|{nonce}|{pairing_code}"
    )
    .into_bytes()
}

pub fn pair_response_canonical(
    server_pubkey_b64: &str,
    client_pubkey_b64: &str,
    ts_ms: i64,
    nonce: &str,
) -> Vec<u8> {
    format!("{PAIR_SERVER_MSG}|{server_pubkey_b64}|{client_pubkey_b64}|{ts_ms}|{nonce}")
        .into_bytes()
}

/// Client verifies `PairResponse.server_signature_b64` against the server public key from the install bundle.
pub fn verify_pair_response(
    server_pubkey_b64_expected: &str,
    client_pubkey_b64: &str,
    ts_ms: i64,
    nonce: &str,
    server_sig_b64: &str,
) -> Result<(), AuthError> {
    let vk = parse_verifying_key_b64(server_pubkey_b64_expected)?;
    let msg = pair_response_canonical(server_pubkey_b64_expected, client_pubkey_b64, ts_ms, nonce);
    verify_sig(&vk, &msg, server_sig_b64)
}

pub fn sign_bytes(sk: &SigningKey, msg: &[u8]) -> String {
    let sig = sk.sign(msg);
    B64.encode(sig.to_bytes())
}

pub fn verify_sig(vk: &VerifyingKey, msg: &[u8], sig_b64: &str) -> Result<(), AuthError> {
    let raw = B64
        .decode(sig_b64.trim())
        .map_err(|e| AuthError::InvalidBase64(e.to_string()))?;
    if raw.len() != 64 {
        return Err(AuthError::BadSignature);
    }
    let arr: [u8; 64] = raw.try_into().map_err(|_| AuthError::BadSignature)?;
    let sig = Signature::from_bytes(&arr);
    vk.verify(msg, &sig).map_err(|_| AuthError::BadSignature)
}

fn normalize_project_for_signing(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("default") {
        "default".to_string()
    } else {
        t.to_string()
    }
}

/// Canonical signed payload for deploy RPCs. Backward compatible: `default` project uses legacy
/// payloads (`""` for unary without extra data; `version` only for Upload/Rollback).
pub fn signing_payload(method: &str, project_id: &str, secondary: &str) -> String {
    let p = normalize_project_for_signing(project_id);
    let is_default = p == "default";
    match method {
        "Upload" | "UploadServerStack" | "Rollback" => {
            if is_default {
                secondary.to_string()
            } else {
                format!("{p}|{secondary}")
            }
        }
        // Same signing payload shape as GetStatus / StopProcess (project only when not default).
        "ProxyTunnel" | "GetStatus" | "StopProcess" | "RestartProcess" | "GetHostStats"
        | "GetHostStatsDetail" | "GetServerStackInfo" => {
            if is_default {
                String::new()
            } else {
                p
            }
        }
        _ => {
            if is_default {
                String::new()
            } else {
                p
            }
        }
    }
}

/// Attach auth metadata for unary/streaming RPC. `project_id` empty means `default`.
/// For `Upload` and `Rollback`, `secondary` is the artifact/target version string; for others it is unused.
pub fn attach_auth_metadata<T>(
    request: &mut tonic::Request<T>,
    sk: &SigningKey,
    method: &str,
    project_id: &str,
    secondary: &str,
) -> Result<(), AuthError> {
    let ts_ms = unix_ms();
    let nonce: String = {
        use rand_core::RngCore;
        format!("{:016x}", OsRng.next_u64())
    };
    let payload = signing_payload(method, project_id, secondary);
    let msg = rpc_canonical(method, ts_ms, &nonce, &payload);
    let sig_b64 = sign_bytes(sk, &msg);

    let m = request.metadata_mut();
    insert_ascii(m, META_PUBKEY, &pubkey_b64_url(sk))?;
    insert_ascii(m, META_TS, &ts_ms.to_string())?;
    insert_ascii(m, META_NONCE, &nonce)?;
    insert_ascii(m, META_SIG, &sig_b64)?;
    if (method == "Upload" || method == "UploadServerStack") && !secondary.is_empty() {
        insert_ascii(m, META_VERSION, secondary)?;
        let p = normalize_project_for_signing(project_id);
        if p != "default" && method == "Upload" {
            insert_ascii(m, META_PROJECT, &p)?;
        }
    }
    Ok(())
}

fn insert_ascii(map: &mut MetadataMap, key: &str, val: &str) -> Result<(), AuthError> {
    let k = tonic::metadata::MetadataKey::from_str(key)
        .map_err(|_| AuthError::InvalidMetadata(key.to_string()))?;
    let v = MetadataValue::<Ascii>::try_from(val)
        .map_err(|_| AuthError::InvalidMetadata(format!("{key} value")))?;
    map.insert(k, v);
    Ok(())
}

fn unix_ms() -> i64 {
    now_unix_ms()
}

/// Current time in milliseconds (for skew checks).
pub fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct AuthConfig {
    pub allow_unauthenticated: bool,
    /// Max |now - ts| in milliseconds for request validation.
    pub max_clock_skew_ms: i64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_unauthenticated: false,
            max_clock_skew_ms: 5 * 60 * 1000,
        }
    }
}

/// Server-side verification of standard RPC metadata.
pub fn verify_rpc_metadata(
    meta: &MetadataMap,
    peers: &HashSet<[u8; 32]>,
    method: &str,
    payload: &str,
    config: &AuthConfig,
    nonce_tracker: &NonceTracker,
) -> Result<(), AuthError> {
    if config.allow_unauthenticated {
        return Ok(());
    }

    let pk = meta
        .get(META_PUBKEY)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingMetadata(META_PUBKEY))?;
    let ts_s = meta
        .get(META_TS)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingMetadata(META_TS))?;
    let nonce = meta
        .get(META_NONCE)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingMetadata(META_NONCE))?;
    let sig = meta
        .get(META_SIG)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingMetadata(META_SIG))?;

    let ts_ms: i64 = ts_s
        .parse()
        .map_err(|_| AuthError::InvalidMetadata("timestamp".into()))?;
    let now = unix_ms();
    if (now - ts_ms).abs() > config.max_clock_skew_ms {
        return Err(AuthError::TimestampSkew);
    }

    nonce_tracker.check_and_insert(ts_ms, nonce)?;

    let vk = parse_verifying_key_b64(pk)?;
    let key_bytes = *vk.as_bytes();
    if !peers.contains(&key_bytes) {
        return Err(AuthError::UnknownPeer);
    }

    let msg = rpc_canonical(method, ts_ms, nonce, payload);
    verify_sig(&vk, &msg, sig)?;
    Ok(())
}

/// For `Upload`, the signed payload matches [`signing_payload`] for `Upload`; version from `x-deploy-version`.
pub fn verify_upload_metadata(
    meta: &MetadataMap,
    peers: &HashSet<[u8; 32]>,
    config: &AuthConfig,
    nonce_tracker: &NonceTracker,
) -> Result<(), AuthError> {
    let ver = meta
        .get(META_VERSION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingMetadata(META_VERSION))?;
    let proj = meta
        .get(META_PROJECT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default");
    let payload = signing_payload("Upload", proj, ver);
    verify_rpc_metadata(meta, peers, "Upload", &payload, config, nonce_tracker)
}

/// Same as [`verify_upload_metadata`] but for `UploadServerStack` (no project metadata).
pub fn verify_upload_server_stack_metadata(
    meta: &MetadataMap,
    peers: &HashSet<[u8; 32]>,
    config: &AuthConfig,
    nonce_tracker: &NonceTracker,
) -> Result<(), AuthError> {
    let ver = meta
        .get(META_VERSION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingMetadata(META_VERSION))?;
    let payload = signing_payload("UploadServerStack", "default", ver);
    verify_rpc_metadata(
        meta,
        peers,
        "UploadServerStack",
        &payload,
        config,
        nonce_tracker,
    )
}

/// Parse connection bundle JSON: `{"token":"...","url":"http://..."}` optional `pairing`.
#[derive(Debug, Clone)]
pub struct ConnectionBundle {
    pub server_pubkey_b64: String,
    pub url: String,
    pub pairing_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BundleJson {
    token: String,
    url: String,
    #[serde(default, alias = "pairing")]
    pairing_code: Option<String>,
}

impl ConnectionBundle {
    pub fn parse(text: &str) -> Result<Self, AuthError> {
        let t = text.trim();
        if t.starts_with('{') {
            let j: BundleJson = serde_json::from_str(t).map_err(|e| {
                AuthError::InvalidMetadata(format!("bundle json: {e}"))
            })?;
            return Ok(ConnectionBundle {
                server_pubkey_b64: j.token,
                url: j.url.trim().to_string(),
                pairing_code: j.pairing_code,
            });
        }
        Err(AuthError::InvalidMetadata(
            "expected JSON bundle with token and url".into(),
        ))
    }
}

/// Build install bundle line for operators.
pub fn format_install_bundle(server_pubkey_b64: &str, url: &str, pairing: &str) -> String {
    serde_json::json!({
        "token": server_pubkey_b64,
        "url": url,
        "pairing": pairing,
    })
    .to_string()
}

/// Generate random pairing code (hex, 32 bytes).
pub fn random_pairing_code() -> String {
    use rand_core::RngCore;
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

pub fn load_or_create_pairing_code(path: &Path) -> Result<String, AuthError> {
    if path.exists() {
        let s = std::fs::read_to_string(path)?;
        let code = s.trim().to_string();
        if !code.is_empty() {
            return Ok(code);
        }
    }
    let code = random_pairing_code();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, format!("{}\n", code))?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_sign_upload() {
        let sk = SigningKey::generate(&mut OsRng);
        let mut peers = HashSet::new();
        peers.insert(*sk.verifying_key().as_bytes());

        let mut req = tonic::Request::new(());
        attach_auth_metadata(&mut req, &sk, "GetStatus", "default", "").unwrap();
        let meta = req.metadata();
        let tracker = NonceTracker::default();
        let cfg = AuthConfig::default();
        verify_rpc_metadata(meta, &peers, "GetStatus", "", &cfg, &tracker).unwrap();
    }

    #[test]
    fn signing_payload_upload_server_stack_uses_version_only_for_default_project() {
        assert_eq!(
            signing_payload("UploadServerStack", "default", "v2026-1"),
            "v2026-1"
        );
    }
}
