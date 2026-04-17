//! Linux: determine whether `pid` has a listening TCP socket on loopback or all interfaces for `port`.
//! Used to avoid false ValidateDeploy blockers when the same project still holds the port.

#[cfg(any(test, target_os = "linux"))]
mod proc_tcp {
    use std::collections::HashSet;

    /// `127.0.0.1` in `/proc/net/tcp` local_address hex form.
    const LOCALHOST_HEX: &str = "0100007F";
    /// `0.0.0.0` — process often listens here; `TcpListener::bind(127.0.0.1:p)` still fails.
    const ANY_HEX: &str = "00000000";

    /// Parse `/proc/net/tcp` content and return inode numbers for LISTEN rows matching localhost or any on `port`.
    pub fn tcp_listen_inodes_for_port_from_text(raw: &str, port: u16) -> HashSet<u64> {
        let port_hex = format!("{:04X}", port);
        let mut out = HashSet::new();
        for line in raw.lines().skip(1) {
            let mut parts = line.split_whitespace();
            let Some(_sl) = parts.next() else {
                continue;
            };
            let Some(local) = parts.next() else {
                continue;
            };
            let Some(_rem) = parts.next() else {
                continue;
            };
            let Some(st) = parts.next() else {
                continue;
            };
            if st != "0A" {
                continue;
            }
            let Some((ip_hex, p_hex)) = local.split_once(':') else {
                continue;
            };
            if !p_hex.eq_ignore_ascii_case(&port_hex) {
                continue;
            }
            if ip_hex != LOCALHOST_HEX && ip_hex != ANY_HEX {
                continue;
            }
            let Some(inode) = parts.last().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            out.insert(inode);
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::proc_tcp::tcp_listen_inodes_for_port_from_text;
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    /// True if `pid` appears to own a LISTEN socket on `port` on 127.0.0.1 or 0.0.0.0.
    pub fn pid_listens_on_deploy_port(pid: u32, port: u16) -> bool {
        let Some(want_inodes) = loopback_any_listen_inodes_for_port(port) else {
            return false;
        };
        if want_inodes.is_empty() {
            return false;
        }
        let Ok(own) = socket_inodes_for_pid(pid) else {
            return false;
        };
        want_inodes.iter().any(|i| own.contains(i))
    }

    fn loopback_any_listen_inodes_for_port(port: u16) -> Option<HashSet<u64>> {
        let raw = fs::read_to_string("/proc/net/tcp").ok()?;
        Some(tcp_listen_inodes_for_port_from_text(&raw, port))
    }

    fn socket_inodes_for_pid(pid: u32) -> std::io::Result<HashSet<u64>> {
        let mut set = HashSet::new();
        let fd_dir = Path::new("/proc").join(pid.to_string()).join("fd");
        let rd = match fs::read_dir(&fd_dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(set),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(set),
            Err(e) => return Err(e),
        };
        for ent in rd.flatten() {
            let link = match fs::read_link(ent.path()) {
                Ok(l) => l,
                Err(_) => continue,
            };
            let s = link.to_string_lossy();
            if let Some(rest) = s.strip_prefix("socket:[") {
                if let Some(num) = rest.strip_suffix(']') {
                    if let Ok(ino) = num.parse::<u64>() {
                        set.insert(ino);
                    }
                }
            }
        }
        Ok(set)
    }
}

#[cfg(target_os = "linux")]
pub use linux::pid_listens_on_deploy_port;

#[cfg(not(target_os = "linux"))]
pub fn pid_listens_on_deploy_port(_pid: u32, _port: u16) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::proc_tcp::tcp_listen_inodes_for_port_from_text;

    #[test]
    fn parses_tcp_line_localhost_and_any_listen() {
        let sample = r"sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1234567
   1: 00000000:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 7654321
";
        let found = tcp_listen_inodes_for_port_from_text(sample, 3000);
        assert!(found.contains(&1234567));
        assert!(found.contains(&7654321));
    }
}
