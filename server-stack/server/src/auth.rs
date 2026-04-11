//! Server-side Ed25519 identity, peer list, and pairing.

use deploy_auth::{
    load_authorized_peers, load_or_create_identity, load_or_create_pairing_code,
    parse_verifying_key_b64, pair_request_canonical, pair_response_canonical, pubkey_b64_url,
    save_authorized_peers, sign_bytes, verify_sig, AuthConfig, NonceTracker,
};
use ed25519_dalek::SigningKey;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tonic::Status;

pub struct ServerAuth {
    pub signing_key: SigningKey,
    pub server_pubkey_b64: String,
    pub peers_path: PathBuf,
    pub peers: Arc<RwLock<HashSet<[u8; 32]>>>,
    pub pairing_code_path: PathBuf,
    pub pairing_code: Arc<RwLock<String>>,
    pub nonce_tracker: NonceTracker,
    pub config: AuthConfig,
}

impl ServerAuth {
    pub fn init(keys_dir: &Path, allow_unauthenticated: bool) -> Result<Option<Arc<Self>>, Box<dyn std::error::Error>> {
        if allow_unauthenticated {
            return Ok(None);
        }
        std::fs::create_dir_all(keys_dir)?;
        let id_path = keys_dir.join("server_ed25519.json");
        let peers_path = keys_dir.join("authorized_peers.json");
        let pairing_path = keys_dir.join("pairing.code");

        let signing_key = load_or_create_identity(&id_path)?;
        let server_pubkey_b64 = pubkey_b64_url(&signing_key);
        let peers = Arc::new(RwLock::new(load_authorized_peers(&peers_path)?));
        let pairing_code = Arc::new(RwLock::new(load_or_create_pairing_code(&pairing_path)?));

        Ok(Some(Arc::new(Self {
            signing_key,
            server_pubkey_b64,
            peers_path,
            peers,
            pairing_code_path: pairing_path,
            pairing_code,
            nonce_tracker: NonceTracker::default(),
            config: AuthConfig::default(),
        })))
    }

    pub fn reload_pairing_code(&self) -> Result<(), std::io::Error> {
        if self.pairing_code_path.exists() {
            let s = std::fs::read_to_string(&self.pairing_code_path)?;
            *self.pairing_code.write() = s.trim().to_string();
        }
        Ok(())
    }

    pub fn verify_pairing(&self, code: &str) -> Result<(), Status> {
        let expected = self.pairing_code.read();
        if expected.is_empty() {
            return Err(Status::permission_denied("pairing code not configured"));
        }
        if code != expected.as_str() {
            return Err(Status::permission_denied("invalid pairing code"));
        }
        Ok(())
    }

    pub fn add_peer(&self, vk_b64: &str) -> Result<(), Status> {
        let vk = parse_verifying_key_b64(vk_b64).map_err(|e| Status::invalid_argument(e.to_string()))?;
        {
            let mut g = self.peers.write();
            g.insert(*vk.as_bytes());
            save_authorized_peers(&self.peers_path, &g).map_err(|e| Status::internal(e.to_string()))?;
        }
        Ok(())
    }
}

pub fn verify_pair_signature(
    client_pub_b64: &str,
    server_pub_b64: &str,
    ts_ms: i64,
    nonce: &str,
    pairing_code: &str,
    client_sig_b64: &str,
) -> Result<(), Status> {
    let vk = parse_verifying_key_b64(client_pub_b64).map_err(|e| Status::invalid_argument(e.to_string()))?;
    let msg = pair_request_canonical(client_pub_b64, server_pub_b64, ts_ms, nonce, pairing_code);
    verify_sig(&vk, &msg, client_sig_b64).map_err(|e| Status::permission_denied(e.to_string()))
}

pub fn sign_pair_response(
    server_sk: &SigningKey,
    server_pub_b64: &str,
    client_pub_b64: &str,
    ts_ms: i64,
    nonce: &str,
) -> String {
    let msg = pair_response_canonical(server_pub_b64, client_pub_b64, ts_ms, nonce);
    sign_bytes(server_sk, &msg)
}
