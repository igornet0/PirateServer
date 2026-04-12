//! Trojan-style auth line (SHA-224 hex) + binary SOCKS5-like address (no outer TLS in Pirate mode).

use sha2::{Digest, Sha224};

const HEX: &[u8; 16] = b"0123456789abcdef";

/// 56-char lowercase hex of SHA-224(password) + `\r\n`.
pub fn trojan_auth_line(password: &str) -> Vec<u8> {
    let mut h = Sha224::new();
    h.update(password.as_bytes());
    let d = h.finalize();
    let mut out = Vec::with_capacity(56 + 2);
    for byte in d {
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0xf) as usize]);
    }
    out.extend_from_slice(b"\r\n");
    out
}

pub fn trojan_parse_and_verify(line_56: &[u8], password: &str) -> bool {
    if line_56.len() != 56 {
        return false;
    }
    let expected = trojan_auth_line(password);
    line_56 == &expected[..56]
}

#[derive(Debug, Clone)]
pub struct TrojanAddr {
    pub host: String,
    pub port: u16,
}

/// After `\r\n` following auth line: SOCKS5-like binary — atype (1) + addr + port (2 BE).
pub fn trojan_parse_address(buf: &[u8]) -> Result<Option<(TrojanAddr, usize)>, crate::WireError> {
    if buf.is_empty() {
        return Ok(None);
    }
    if buf.len() < 1 + 2 {
        return Ok(None);
    }
    let atype = buf[0];
    match atype {
        1 => {
            if buf.len() < 1 + 4 + 2 {
                return Ok(None);
            }
            let ip = format!("{}.{}.{}.{}", buf[1], buf[2], buf[3], buf[4]);
            let port = u16::from_be_bytes([buf[5], buf[6]]);
            Ok(Some((TrojanAddr { host: ip, port }, 7)))
        }
        3 => {
            if buf.len() < 2 {
                return Ok(None);
            }
            let n = buf[1] as usize;
            if buf.len() < 2 + n + 2 {
                return Ok(None);
            }
            let host = std::str::from_utf8(&buf[2..2 + n])
                .map_err(|e| crate::WireError::Parse(e.to_string()))?
                .to_string();
            let port = u16::from_be_bytes([buf[2 + n], buf[2 + n + 1]]);
            Ok(Some((TrojanAddr { host, port }, 2 + n + 2)))
        }
        4 => {
            if buf.len() < 1 + 16 + 2 {
                return Ok(None);
            }
            let mut a = [0u8; 16];
            a.copy_from_slice(&buf[1..17]);
            let host = std::net::Ipv6Addr::from(a).to_string();
            let port = u16::from_be_bytes([buf[17], buf[18]]);
            Ok(Some((TrojanAddr { host, port }, 19)))
        }
        _ => Err(crate::WireError::Protocol("unknown trojan address type".into())),
    }
}

pub fn trojan_build_address(addr: &TrojanAddr) -> Result<Vec<u8>, crate::WireError> {
    let ip4: Option<std::net::Ipv4Addr> = addr.host.parse().ok();
    if let Some(ip) = ip4 {
        let o = ip.octets();
        let mut v = vec![1u8];
        v.extend_from_slice(&o);
        v.extend_from_slice(&addr.port.to_be_bytes());
        return Ok(v);
    }
    let ip6: Option<std::net::Ipv6Addr> = addr.host.parse().ok();
    if let Some(ip) = ip6 {
        let mut v = vec![4u8];
        v.extend_from_slice(&ip.octets());
        v.extend_from_slice(&addr.port.to_be_bytes());
        return Ok(v);
    }
    let b = addr.host.as_bytes();
    if b.len() > 255 {
        return Err(crate::WireError::Protocol("domain too long".into()));
    }
    let mut v = vec![3u8, b.len() as u8];
    v.extend_from_slice(b);
    v.extend_from_slice(&addr.port.to_be_bytes());
    Ok(v)
}

pub enum TrojanHandshakeResult {
    NeedMore(usize),
    Ready {
        addr: TrojanAddr,
        payload_offset: usize,
    },
    InvalidAuth,
}

/// Buffer may contain partial data; returns Ready when auth + address parsed.
pub fn trojan_server_handshake(buf: &[u8], password: &str) -> TrojanHandshakeResult {
    // Find end of first line (auth 56 + \r\n)
    let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") else {
        return TrojanHandshakeResult::NeedMore(58);
    };
    if pos < 56 {
        return TrojanHandshakeResult::NeedMore(58);
    }
    let line = &buf[..pos];
    if line.len() != 56 || !trojan_parse_and_verify(line, password) {
        return TrojanHandshakeResult::InvalidAuth;
    }
    let after = pos + 2;
    if buf.len() <= after {
        return TrojanHandshakeResult::NeedMore(after + 7);
    }
    match trojan_parse_address(&buf[after..]) {
        Ok(Some((addr, consumed))) => TrojanHandshakeResult::Ready {
            addr,
            payload_offset: after + consumed,
        },
        Ok(None) => TrojanHandshakeResult::NeedMore(after + 7),
        Err(_) => TrojanHandshakeResult::InvalidAuth,
    }
}
