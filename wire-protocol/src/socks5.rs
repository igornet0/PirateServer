//! SOCKS5 server-side parsing for Pirate `ProxyTunnel` (pipelined greeting + CONNECT).

#[derive(Debug, Clone)]
pub struct Socks5Target {
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub enum Socks5ServerHandshake {
    NeedMore(usize),
    Ready {
        target: Socks5Target,
        consumed: usize,
    },
    Invalid(&'static str),
}

fn read_addr(buf: &[u8], off: usize) -> Option<(String, usize, u16)> {
    if off >= buf.len() {
        return None;
    }
    match buf[off] {
        1 => {
            if buf.len() < off + 7 {
                return None;
            }
            let ip = format!(
                "{}.{}.{}.{}",
                buf[off + 1],
                buf[off + 2],
                buf[off + 3],
                buf[off + 4]
            );
            let port = u16::from_be_bytes([buf[off + 5], buf[off + 6]]);
            Some((ip, off + 7, port))
        }
        3 => {
            let n = *buf.get(off + 1)? as usize;
            if buf.len() < off + 2 + n + 2 {
                return None;
            }
            let host = String::from_utf8_lossy(&buf[off + 2..off + 2 + n]).to_string();
            let port = u16::from_be_bytes([buf[off + 2 + n], buf[off + 2 + n + 1]]);
            Some((host, off + 2 + n + 2, port))
        }
        4 => {
            if buf.len() < off + 19 {
                return None;
            }
            let mut a = [0u8; 16];
            a.copy_from_slice(&buf[off + 1..off + 17]);
            let ip = std::net::Ipv6Addr::from(a);
            let port = u16::from_be_bytes([buf[off + 17], buf[off + 18]]);
            Some((format!("{ip}"), off + 19, port))
        }
        _ => None,
    }
}

/// Parse pipelined: `[ver=5,nmeth,methods...][ver=5,cmd,rsv,atyp,...]` or with RFC1929 between.
pub fn socks5_server_parse(
    buf: &[u8],
    require_auth: bool,
    user: Option<&str>,
    pass: Option<&str>,
) -> Socks5ServerHandshake {
    if buf.len() < 3 {
        return Socks5ServerHandshake::NeedMore(3);
    }
    if buf[0] != 5 {
        return Socks5ServerHandshake::Invalid("socks5: bad version");
    }
    let nmeth = buf[1] as usize;
    if buf.len() < 2 + nmeth {
        return Socks5ServerHandshake::NeedMore(2 + nmeth);
    }
    let mut off = 2 + nmeth;
    if require_auth {
        if !buf[2..2 + nmeth].iter().any(|&m| m == 2) {
            return Socks5ServerHandshake::Invalid("socks5: username auth required");
        }
        if buf.len() < off + 3 {
            return Socks5ServerHandshake::NeedMore(off + 3);
        }
        if buf[off] != 1 {
            return Socks5ServerHandshake::Invalid("socks5: expected RFC1929 auth");
        }
        let ulen = buf[off + 1] as usize;
        if buf.len() < off + 3 + ulen {
            return Socks5ServerHandshake::NeedMore(off + 3 + ulen);
        }
        let plen = buf[off + 2 + ulen] as usize;
        if buf.len() < off + 3 + ulen + plen {
            return Socks5ServerHandshake::NeedMore(off + 3 + ulen + plen);
        }
        let u = std::str::from_utf8(&buf[off + 2..off + 2 + ulen]).unwrap_or("");
        let p = std::str::from_utf8(&buf[off + 3 + ulen..off + 3 + ulen + plen]).unwrap_or("");
        if user.map(str::trim) != Some(u.trim()) || pass.map(str::trim) != Some(p.trim()) {
            return Socks5ServerHandshake::Invalid("socks5: auth failed");
        }
        off = off + 3 + ulen + plen;
    } else if !buf[2..2 + nmeth].iter().any(|&m| m == 0) {
        return Socks5ServerHandshake::Invalid("socks5: no-auth method missing");
    }

    if buf.len() < off + 4 {
        return Socks5ServerHandshake::NeedMore(off + 4);
    }
    if buf[off] != 5 {
        return Socks5ServerHandshake::Invalid("socks5: bad request version");
    }
    if buf[off + 1] != 1 {
        return Socks5ServerHandshake::Invalid("socks5: only CONNECT");
    }
    let Some((host, end, port)) = read_addr(buf, off + 3) else {
        return Socks5ServerHandshake::NeedMore(buf.len().saturating_add(256));
    };
    Socks5ServerHandshake::Ready {
        target: Socks5Target { host, port },
        consumed: end,
    }
}

/// Pipelined greeting (no auth) + CONNECT.
pub fn socks5_build_pipeline_connect(host: &str, port: u16) -> Result<Vec<u8>, crate::WireError> {
    let mut out = Vec::new();
    out.extend_from_slice(&[5, 1, 0]);
    out.push(5);
    out.push(1);
    out.push(0);
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        out.push(1);
        out.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        out.push(4);
        out.extend_from_slice(&ip.octets());
    } else {
        let b = host.as_bytes();
        if b.len() > 255 {
            return Err(crate::WireError::Protocol("socks5 domain too long".into()));
        }
        out.push(3);
        out.push(b.len() as u8);
        out.extend_from_slice(b);
    }
    out.extend_from_slice(&port.to_be_bytes());
    Ok(out)
}
