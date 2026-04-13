//! Shadowsocks AEAD (TCP) helpers using `shadowsocks-crypto` v1.

use rand::RngCore;
use shadowsocks_crypto::kind::CipherKind;
use shadowsocks_crypto::v1::{openssl_bytes_to_key, Cipher};

use crate::trojan::{trojan_parse_address, TrojanAddr};

#[derive(Debug)]
pub enum SsTcpHandshakeResult {
    NeedMore(usize),
    Ready {
        addr: TrojanAddr,
        /// Byte offset in original `buf` after consumed handshake (salt + encrypted chunks).
        consumed: usize,
        /// Extra plaintext after address inside first payload (if any).
        tail_after_addr: Vec<u8>,
    },
    Invalid(&'static str),
}

/// Decode first AEAD TCP segments after salt; `buf` must start with salt (caller slices).
fn decrypt_first_payload(
    buf: &[u8],
    method: &str,
    password: &str,
) -> Result<Option<(TrojanAddr, usize, Vec<u8>)>, crate::WireError> {
    let kind: CipherKind = method.trim().parse().map_err(|_| {
        crate::WireError::Protocol("shadowsocks: unknown cipher method".into())
    })?;
    if !kind.is_aead() {
        return Err(crate::WireError::Protocol(
            "shadowsocks: only AEAD methods supported on Pirate wire".into(),
        ));
    }
    let salt_len = kind.salt_len();
    let tag_len = kind.tag_len();
    if buf.len() < salt_len {
        return Ok(None);
    }
    let salt = &buf[..salt_len];
    let mut key = vec![0u8; kind.key_len()];
    openssl_bytes_to_key(password.as_bytes(), &mut key);
    let mut cipher = Cipher::new(kind, &key, salt);
    let mut off = salt_len;
    let len_ct_len = 2 + tag_len;
    if buf.len() < off + len_ct_len {
        return Ok(None);
    }
    let mut len_pkt = buf[off..off + len_ct_len].to_vec();
    if !cipher.decrypt_packet(&mut len_pkt) {
        return Err(crate::WireError::Protocol(
            "shadowsocks: length chunk decrypt failed".into(),
        ));
    }
    if len_pkt.len() < 2 {
        return Err(crate::WireError::Protocol(
            "shadowsocks: bad length plaintext".into(),
        ));
    }
    let plen = u16::from_be_bytes([len_pkt[0], len_pkt[1]]) as usize;
    off += len_ct_len;
    let pay_ct_len = plen + tag_len;
    if buf.len() < off + pay_ct_len {
        return Ok(None);
    }
    let mut pay = buf[off..off + pay_ct_len].to_vec();
    if !cipher.decrypt_packet(&mut pay) {
        return Err(crate::WireError::Protocol(
            "shadowsocks: payload decrypt failed".into(),
        ));
    }
    off += pay_ct_len;
    let addr_part = trojan_parse_address(&pay)?;
    let Some((addr, n)) = addr_part else {
        return Err(crate::WireError::Protocol(
            "shadowsocks: incomplete address".into(),
        ));
    };
    let tail = pay[n..].to_vec();
    Ok(Some((addr, off, tail)))
}

/// Server: `buf` is accumulated client bytes (starts with salt).
pub fn ss_tcp_server_handshake(buf: &[u8], method: &str, password: &str) -> SsTcpHandshakeResult {
    let kind: CipherKind = match method.trim().parse() {
        Ok(k) => k,
        Err(_) => return SsTcpHandshakeResult::Invalid("bad cipher method"),
    };
    if !kind.is_aead() {
        return SsTcpHandshakeResult::Invalid("only AEAD");
    }
    let salt_len = kind.salt_len();
    let tag_len = kind.tag_len();
    // Minimum first payload: IPv4 address (7 bytes) + AEAD tag.
    let min_payload_ct = 7 + tag_len;
    let min_need = salt_len + (2 + tag_len) + min_payload_ct;
    if buf.len() < salt_len + (2 + tag_len) {
        return SsTcpHandshakeResult::NeedMore(salt_len + (2 + tag_len));
    }
    match decrypt_first_payload(buf, method, password) {
        Ok(None) => SsTcpHandshakeResult::NeedMore(min_need.max(buf.len() + 32)),
        Ok(Some((addr, consumed, tail))) => SsTcpHandshakeResult::Ready {
            addr,
            consumed,
            tail_after_addr: tail,
        },
        Err(_) => SsTcpHandshakeResult::Invalid("decrypt"),
    }
}

/// Client: build salt + encrypted length + encrypted payload (address + tail).
pub fn ss_client_first_chunk(
    method: &str,
    password: &str,
    target: &TrojanAddr,
    tail: &[u8],
) -> Result<Vec<u8>, crate::WireError> {
    let kind: CipherKind = method
        .trim()
        .parse()
        .map_err(|_| crate::WireError::Protocol("shadowsocks: bad method".into()))?;
    if !kind.is_aead() {
        return Err(crate::WireError::Protocol(
            "shadowsocks: only AEAD".into(),
        ));
    }
    let salt_len = kind.salt_len();
    let tag_len = kind.tag_len();
    let mut key = vec![0u8; kind.key_len()];
    openssl_bytes_to_key(password.as_bytes(), &mut key);
    let mut salt = vec![0u8; salt_len];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut cipher = Cipher::new(kind, &key, &salt);
    let mut plain = Vec::new();
    use crate::trojan::trojan_build_address;
    plain.extend_from_slice(&trojan_build_address(target)?);
    plain.extend_from_slice(tail);
    let plen = plain.len() as u16;
    let mut len_plain = plen.to_be_bytes().to_vec();
    let len_ct_size = len_plain.len() + tag_len;
    len_plain.resize(len_ct_size, 0);
    cipher.encrypt_packet(&mut len_plain);
    let mut pay = plain;
    let pay_sz = pay.len() + tag_len;
    pay.resize(pay_sz, 0);
    cipher.encrypt_packet(&mut pay);
    let mut out = salt;
    out.extend_from_slice(&len_plain);
    out.extend_from_slice(&pay);
    Ok(out)
}
