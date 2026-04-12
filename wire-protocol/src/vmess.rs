//! Minimal VMess AEAD-like framing: AES-128-GCM header + chunked payload.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use rand::RngCore;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::vless::VlessAddress;

const GCM_NONCE_LEN: usize = 12;
const HEADER_PLAIN_MAX: usize = 256;

fn vmess_key(uuid: &uuid::Uuid) -> [u8; 16] {
    let d = md5::compute(uuid.as_bytes());
    let mut k = [0u8; 16];
    k.copy_from_slice(d.as_slice());
    k
}

/// Client builds encrypted open header: nonce(12) + ciphertext(tag included in aes-gcm).
pub fn vmess_client_header(uuid: &uuid::Uuid, port: u16, addr: &VlessAddress) -> Result<Vec<u8>, crate::WireError> {
    let key = vmess_key(uuid);
    let cipher = Aes128Gcm::new_from_slice(&key)
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    let mut plain = Vec::new();
    let ts = chrono::Utc::now().timestamp();
    plain.extend_from_slice(&ts.to_be_bytes());
    plain.push(1u8); // TCP
    plain.extend_from_slice(&port.to_be_bytes());
    match addr {
        VlessAddress::IpV4(b) => {
            plain.push(1);
            plain.extend_from_slice(b);
        }
        VlessAddress::Domain(s) => {
            plain.push(2);
            let b = s.as_bytes();
            plain.push(b.len().min(255) as u8);
            plain.extend_from_slice(&b[..b.len().min(255)]);
        }
        VlessAddress::IpV6(b) => {
            plain.push(3);
            plain.extend_from_slice(b);
        }
    }
    if plain.len() > HEADER_PLAIN_MAX {
        return Err(crate::WireError::Protocol("vmess header too large".into()));
    }
    let mut nonce = [0u8; GCM_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let n = Nonce::from_slice(&nonce);
    let ct = cipher
        .encrypt(n, plain.as_ref())
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    let mut out = Vec::with_capacity(nonce.len() + 2 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&(ct.len() as u16).to_be_bytes());
    out.extend_from_slice(&ct);
    Ok(out)
}

pub struct VMessHeaderOpen {
    pub port: u16,
    pub addr: VlessAddress,
    pub timestamp: i64,
}

/// Full encoded header length when `buf` has at least 14 bytes and enough for ciphertext.
pub fn vmess_open_header_byte_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < GCM_NONCE_LEN + 2 {
        return None;
    }
    let ct_len = u16::from_be_bytes([buf[GCM_NONCE_LEN], buf[GCM_NONCE_LEN + 1]]) as usize;
    Some(GCM_NONCE_LEN + 2 + ct_len)
}

/// Decrypt open header from client (`nonce(12) + ct_len(2) + ciphertext`).
pub fn vmess_server_open_header(uuid: &uuid::Uuid, buf: &[u8]) -> Result<Option<VMessHeaderOpen>, crate::WireError> {
    if buf.len() < GCM_NONCE_LEN + 2 {
        return Ok(None);
    }
    let ct_len = u16::from_be_bytes([buf[GCM_NONCE_LEN], buf[GCM_NONCE_LEN + 1]]) as usize;
    if buf.len() < GCM_NONCE_LEN + 2 + ct_len {
        return Ok(None);
    }
    let key = vmess_key(uuid);
    let cipher = Aes128Gcm::new_from_slice(&key)
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    let nonce = Nonce::from_slice(&buf[..GCM_NONCE_LEN]);
    let ct = &buf[GCM_NONCE_LEN + 2..GCM_NONCE_LEN + 2 + ct_len];
    let plain = cipher
        .decrypt(nonce, ct.as_ref())
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    if plain.len() < 8 + 1 + 2 + 1 {
        return Err(crate::WireError::Protocol("vmess plain too short".into()));
    }
    let now = chrono::Utc::now().timestamp();
    let ts = i64::from_be_bytes(plain[0..8].try_into().unwrap());
    if (now - ts).abs() > 120 {
        return Err(crate::WireError::Protocol("vmess timestamp out of window".into()));
    }
    let cmd = plain[8];
    if cmd != 1 {
        return Err(crate::WireError::Protocol("only TCP cmd supported".into()));
    }
    let port = u16::from_be_bytes([plain[9], plain[10]]);
    let atype = plain[11];
    let (addr, _) = crate::vless::read_address(&plain[12..], atype)
        .ok_or_else(|| crate::WireError::Protocol("bad vmess addr".into()))?;
    Ok(Some(VMessHeaderOpen {
        port,
        addr,
        timestamp: i64::from_be_bytes(plain[0..8].try_into().unwrap()),
    }))
}

/// Chunk: nonce(12) + len(2 BE) + ciphertext(len bytes plain, max chunk).
pub fn vmess_aead_encode_chunk(uuid: &uuid::Uuid, plain: &[u8]) -> Result<Vec<u8>, crate::WireError> {
    if plain.len() > 0xffff {
        return Err(crate::WireError::Protocol("chunk too large".into()));
    }
    let key = vmess_key(uuid);
    let cipher = Aes128Gcm::new_from_slice(&key)
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    let mut nonce = [0u8; GCM_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let n = Nonce::from_slice(&nonce);
    let ct = cipher
        .encrypt(n, plain.as_ref())
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    let mut out = Vec::with_capacity(12 + 2 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&(plain.len() as u16).to_be_bytes());
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn vmess_aead_decode_chunk(uuid: &uuid::Uuid, buf: &[u8]) -> Result<Option<(Vec<u8>, usize)>, crate::WireError> {
    if buf.len() < GCM_NONCE_LEN + 2 {
        return Ok(None);
    }
    let key = vmess_key(uuid);
    let cipher = Aes128Gcm::new_from_slice(&key)
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    let nonce = Nonce::from_slice(&buf[..GCM_NONCE_LEN]);
    let plain_len = u16::from_be_bytes([buf[GCM_NONCE_LEN], buf[GCM_NONCE_LEN + 1]]) as usize;
    let ct_start = GCM_NONCE_LEN + 2;
    let ct_len = plain_len + 16; // GCM tag
    if buf.len() < ct_start + ct_len {
        return Ok(None);
    }
    let ct = &buf[ct_start..ct_start + ct_len];
    let plain = cipher
        .decrypt(nonce, ct.as_ref())
        .map_err(|e| crate::WireError::Crypto(e.to_string()))?;
    if plain.len() != plain_len {
        return Err(crate::WireError::Protocol("vmess chunk len mismatch".into()));
    }
    let total = ct_start + ct_len;
    Ok(Some((plain, total)))
}

/// Replay guard: reject duplicate nonces within TTL (best-effort).
pub struct VmessReplayCache {
    ttl: Duration,
    max_entries: usize,
    q: VecDeque<(Vec<u8>, Instant)>,
    set: HashMap<Vec<u8>, Instant>,
}

impl VmessReplayCache {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs.max(30)),
            max_entries,
            q: VecDeque::new(),
            set: HashMap::new(),
        }
    }

    pub fn check_and_insert(&mut self, nonce: &[u8]) -> bool {
        let now = Instant::now();
        while let Some((_, t)) = self.q.front() {
            if now.duration_since(*t) > self.ttl {
                let (n, _) = self.q.pop_front().unwrap();
                self.set.remove(&n);
            } else {
                break;
            }
        }
        let n = nonce.to_vec();
        if self.set.contains_key(&n) {
            return false;
        }
        if self.set.len() >= self.max_entries {
            if let Some((old, _)) = self.q.pop_front() {
                self.set.remove(&old);
            }
        }
        self.set.insert(n.clone(), now);
        self.q.push_back((n, now));
        true
    }
}

pub fn vmess_check_replay(cache: &mut VmessReplayCache, nonce: &[u8]) -> bool {
    cache.check_and_insert(nonce)
}
