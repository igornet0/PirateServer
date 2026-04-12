//! Minimal VLESS framing (version 0) for TCP relay.

use uuid::Uuid;

pub const VLESS_VERSION: u8 = 0;
const CMD_TCP: u8 = 0x01;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VlessAddress {
    IpV4([u8; 4]),
    Domain(String),
    IpV6([u8; 16]),
}

impl VlessAddress {
    pub fn host_string(&self) -> String {
        match self {
            VlessAddress::IpV4(b) => format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]),
            VlessAddress::Domain(s) => s.clone(),
            VlessAddress::IpV6(b) => {
                let a: [u8; 16] = *b;
                std::net::Ipv6Addr::from(a).to_string()
            }
        }
    }
}

pub enum VlessParseResult<'a> {
    /// Need more bytes.
    NeedMore(usize),
    /// Parsed request; payload starts at offset in buffer.
    Ok {
        uuid_ok: bool,
        port: u16,
        addr: VlessAddress,
        payload: &'a [u8],
        /// Byte offset where payload starts (header length).
        payload_start: usize,
    },
    Invalid,
}

pub(crate) fn read_address(buf: &[u8], atype: u8) -> Option<(VlessAddress, usize)> {
    match atype {
        1 if buf.len() >= 4 => {
            let mut a = [0u8; 4];
            a.copy_from_slice(&buf[..4]);
            Some((VlessAddress::IpV4(a), 4))
        }
        2 if !buf.is_empty() => {
            let n = buf[0] as usize;
            if buf.len() < 1 + n {
                return None;
            }
            let s = std::str::from_utf8(&buf[1..1 + n]).ok()?.to_string();
            Some((VlessAddress::Domain(s), 1 + n))
        }
        3 if buf.len() >= 16 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(&buf[..16]);
            Some((VlessAddress::IpV6(a), 16))
        }
        _ => None,
    }
}

/// Parse VLESS request from buffer. `expected_uuid` if Some must match request UUID.
pub fn vless_parse_request<'a>(buf: &'a [u8], expected_uuid: Option<&Uuid>) -> VlessParseResult<'a> {
    if buf.len() < 1 + 16 + 1 + 1 + 2 + 1 {
        return VlessParseResult::NeedMore(1 + 16 + 1 + 1 + 2 + 1);
    }
    if buf[0] != VLESS_VERSION {
        return VlessParseResult::Invalid;
    }
    let mut uuid_bytes = [0u8; 16];
    uuid_bytes.copy_from_slice(&buf[1..17]);
    let _req_uuid = Uuid::from_bytes(uuid_bytes);
    let addons_len = buf[17] as usize;
    let header_after_uuid = 18 + addons_len;
    if buf.len() < header_after_uuid + 1 + 2 + 1 {
        return VlessParseResult::NeedMore(header_after_uuid + 1 + 2 + 1);
    }
    let cmd = buf[header_after_uuid];
    if cmd != CMD_TCP {
        return VlessParseResult::Invalid;
    }
    let port = u16::from_be_bytes([buf[header_after_uuid + 1], buf[header_after_uuid + 2]]);
    let atype = buf[header_after_uuid + 3];
    let addr_start = header_after_uuid + 4;
    if buf.len() < addr_start + 1 {
        return VlessParseResult::NeedMore(addr_start + 1);
    }
    let Some((addr, alen)) = read_address(&buf[addr_start..], atype) else {
        return VlessParseResult::NeedMore(buf.len() + 64);
    };
    let payload_off = addr_start + alen;
    if buf.len() < payload_off {
        return VlessParseResult::NeedMore(payload_off);
    }
    let uuid_ok = expected_uuid.map(|e| e.as_bytes() == &uuid_bytes).unwrap_or(true);
    VlessParseResult::Ok {
        uuid_ok,
        port,
        addr,
        payload: &buf[payload_off..],
        payload_start: payload_off,
    }
}

/// Build VLESS TCP request (version 0, no addons).
pub fn vless_build_request(uuid: &Uuid, port: u16, addr: &VlessAddress, initial_payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + initial_payload.len());
    out.push(VLESS_VERSION);
    out.extend_from_slice(uuid.as_bytes());
    out.push(0u8); // addons len
    out.push(CMD_TCP);
    out.extend_from_slice(&port.to_be_bytes());
    match addr {
        VlessAddress::IpV4(b) => {
            out.push(1);
            out.extend_from_slice(b);
        }
        VlessAddress::Domain(s) => {
            out.push(2);
            let b = s.as_bytes();
            out.push(b.len().min(255) as u8);
            out.extend_from_slice(&b[..b.len().min(255)]);
        }
        VlessAddress::IpV6(b) => {
            out.push(3);
            out.extend_from_slice(b);
        }
    }
    out.extend_from_slice(initial_payload);
    out
}
