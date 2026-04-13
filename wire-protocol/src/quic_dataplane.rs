//! Minimal binary framing for QUIC data-plane streams (Pirate proxy).

use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"PQDP";
pub const FRAME_VERSION: u8 = 1;

pub const CMD_CONNECT: u8 = 1;
pub const CMD_HEALTH_CHECK: u8 = 2;
/// Reserved for future SOCKS-like UDP relay.
pub const CMD_UDP_ASSOCIATE: u8 = 3;

pub const ADDR_IPV4: u8 = 1;
pub const ADDR_DOMAIN: u8 = 2;
pub const ADDR_IPV6: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInitFrame {
    pub command: u8,
    pub ticket: Vec<u8>,
    pub addr_type: u8,
    pub addr: Vec<u8>,
    pub port: u16,
}

#[derive(Debug, Error)]
pub enum QuicDataPlaneError {
    #[error("frame too short")]
    TooShort,
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported version {0}")]
    UnsupportedVersion(u8),
    #[error("ticket too long")]
    TicketTooLong,
    #[error("invalid address")]
    InvalidAddress,
    #[error("unknown command {0}")]
    UnknownCommand(u8),
}

impl StreamInitFrame {
    pub fn encode(&self) -> Result<Vec<u8>, QuicDataPlaneError> {
        if self.ticket.len() > u16::MAX as usize {
            return Err(QuicDataPlaneError::TicketTooLong);
        }
        let mut out = Vec::with_capacity(4 + 1 + 2 + self.ticket.len() + 1 + 1 + 1 + self.addr.len() + 2);
        out.extend_from_slice(MAGIC);
        out.push(FRAME_VERSION);
        let tl = self.ticket.len() as u16;
        out.extend_from_slice(&tl.to_be_bytes());
        out.extend_from_slice(&self.ticket);
        out.push(self.command);
        out.push(self.addr_type);
        let al = self.addr.len() as u8;
        if al as usize != self.addr.len() {
            return Err(QuicDataPlaneError::InvalidAddress);
        }
        out.push(al);
        out.extend_from_slice(&self.addr);
        out.extend_from_slice(&self.port.to_be_bytes());
        Ok(out)
    }

    /// Minimum buffer length to read a full frame (after at least 7 + ticket_len + 3 bytes for addr_len).
    pub fn wire_len(buf: &[u8]) -> Option<usize> {
        if buf.len() < 7 {
            return None;
        }
        if &buf[0..4] != MAGIC {
            return None;
        }
        let tl = u16::from_be_bytes([buf[5], buf[6]]) as usize;
        let base = 7 + tl;
        if buf.len() < base + 3 {
            return None;
        }
        let addr_len = buf[base + 2] as usize;
        Some(base + 3 + addr_len + 2)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, QuicDataPlaneError> {
        if buf.len() < 4 + 1 + 2 {
            return Err(QuicDataPlaneError::TooShort);
        }
        if &buf[0..4] != MAGIC {
            return Err(QuicDataPlaneError::BadMagic);
        }
        let ver = buf[4];
        if ver != FRAME_VERSION {
            return Err(QuicDataPlaneError::UnsupportedVersion(ver));
        }
        let tl = u16::from_be_bytes([buf[5], buf[6]]) as usize;
        let mut i = 7;
        if buf.len() < i + tl + 1 + 1 + 1 + 2 {
            return Err(QuicDataPlaneError::TooShort);
        }
        let ticket = buf[i..i + tl].to_vec();
        i += tl;
        let command = buf[i];
        i += 1;
        let addr_type = buf[i];
        i += 1;
        let addr_len = buf[i] as usize;
        i += 1;
        if buf.len() < i + addr_len + 2 {
            return Err(QuicDataPlaneError::TooShort);
        }
        let addr = buf[i..i + addr_len].to_vec();
        i += addr_len;
        let port = u16::from_be_bytes([buf[i], buf[i + 1]]);
        Ok(Self {
            command,
            ticket,
            addr_type,
            addr,
            port,
        })
    }
}

/// Server → client: one byte status after init (0 = ok).
pub const ACK_OK: u8 = 0;
pub const ACK_ERR: u8 = 1;

pub fn encode_ack(ok: bool, err_msg: Option<&str>) -> Vec<u8> {
    if ok {
        return vec![ACK_OK];
    }
    let msg = err_msg.unwrap_or("error").as_bytes();
    let mut v = Vec::with_capacity(3 + msg.len());
    v.push(ACK_ERR);
    let len = (msg.len().min(u16::MAX as usize)) as u16;
    v.extend_from_slice(&len.to_be_bytes());
    v.extend_from_slice(&msg[..len as usize]);
    v
}

#[derive(Debug, Error)]
pub enum AckError {
    #[error("short ack")]
    Short,
    #[error("upstream error: {0}")]
    Upstream(String),
}

pub fn decode_ack(buf: &[u8]) -> Result<(), AckError> {
    if buf.is_empty() {
        return Err(AckError::Short);
    }
    match buf[0] {
        ACK_OK => Ok(()),
        ACK_ERR => {
            if buf.len() < 3 {
                return Err(AckError::Short);
            }
            let len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
            let s = String::from_utf8_lossy(&buf[3..3 + len.min(buf.len().saturating_sub(3))]);
            Err(AckError::Upstream(s.into_owned()))
        }
        _ => Err(AckError::Upstream("bad ack byte".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_init() {
        let f = StreamInitFrame {
            command: CMD_CONNECT,
            ticket: vec![1, 2, 3, 4],
            addr_type: ADDR_DOMAIN,
            addr: b"example.com".to_vec(),
            port: 443,
        };
        let b = f.encode().unwrap();
        let d = StreamInitFrame::decode(&b).unwrap();
        assert_eq!(f, d);
    }
}
