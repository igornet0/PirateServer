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

    /// Parse `/proc/net/tcp6` — LISTEN rows for `port` (Node often binds `[::]:port` only).
    pub fn tcp6_listen_inodes_for_port_from_text(raw: &str, port: u16) -> HashSet<u64> {
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
            let Some((_, p_hex)) = local.rsplit_once(':') else {
                continue;
            };
            if !p_hex.eq_ignore_ascii_case(&port_hex) {
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
    use super::proc_tcp::{tcp6_listen_inodes_for_port_from_text, tcp_listen_inodes_for_port_from_text};
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    /// True if `pid` appears to own a LISTEN socket on `port` on 127.0.0.1 or 0.0.0.0.
    pub fn pid_listens_on_deploy_port(pid: u32, port: u16) -> bool {
        let want_inodes = loopback_any_listen_inodes_for_port(port);
        if want_inodes.is_empty() {
            return false;
        }
        let Ok(own) = socket_inodes_for_pid(pid) else {
            return false;
        };
        want_inodes.iter().any(|i| own.contains(i))
    }

    /// Inodes for LISTEN on `port` (IPv4 localhost/any + IPv6).
    fn loopback_any_listen_inodes_for_port(port: u16) -> HashSet<u64> {
        let mut set = HashSet::new();
        if let Ok(raw) = fs::read_to_string("/proc/net/tcp") {
            set.extend(tcp_listen_inodes_for_port_from_text(&raw, port));
        }
        if let Ok(raw) = fs::read_to_string("/proc/net/tcp6") {
            set.extend(tcp6_listen_inodes_for_port_from_text(&raw, port));
        }
        set
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

    /// PIDs that currently have a LISTEN socket on `port` (loopback or any).
    pub fn listener_pids_for_port(port: u16) -> Vec<u32> {
        let want_inodes = loopback_any_listen_inodes_for_port(port);
        if want_inodes.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir("/proc") else {
            return out;
        };
        for ent in rd.flatten() {
            let name = ent.file_name();
            let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(own) = socket_inodes_for_pid(pid) else {
                continue;
            };
            if want_inodes.iter().any(|i| own.contains(i)) {
                out.push(pid);
            }
        }
        out
    }

    /// `true` if `pid`'s cwd is under `deploy_root` (same deployed tree as this project).
    pub fn pid_cwd_starts_with_deploy_root(pid: u32, deploy_root: &Path) -> bool {
        let Ok(prefix) = deploy_root.canonicalize() else {
            return false;
        };
        let cwd_link = Path::new("/proc").join(pid.to_string()).join("cwd");
        let Ok(resolved) = fs::read_link(&cwd_link) else {
            return false;
        };
        let Ok(canon) = resolved.canonicalize() else {
            return false;
        };
        canon.starts_with(&prefix)
    }

    /// True if `/proc/pid/cmdline` or `/proc/pid/exe` path mentions the canonical deploy root (Node may have cwd `/`).
    pub fn pid_cmdline_or_exe_contains_deploy_root(pid: u32, deploy_root: &Path) -> bool {
        let Ok(prefix) = deploy_root.canonicalize() else {
            return false;
        };
        let ps = prefix.to_string_lossy();
        if ps.is_empty() {
            return false;
        }
        let cmd_path = Path::new("/proc").join(pid.to_string()).join("cmdline");
        if let Ok(raw) = fs::read(&cmd_path) {
            if String::from_utf8_lossy(&raw).contains(ps.as_ref()) {
                return true;
            }
        }
        let exe_link = Path::new("/proc").join(pid.to_string()).join("exe");
        if let Ok(exe) = fs::read_link(&exe_link) {
            if exe.to_string_lossy().contains(ps.as_ref()) {
                return true;
            }
        }
        false
    }

    /// True if any regular file fd points inside `deploy_root` (Node often keeps `server.js` open while cwd is `/`).
    pub fn pid_has_open_file_under_deploy_root(pid: u32, deploy_root: &Path) -> bool {
        let Ok(prefix) = deploy_root.canonicalize() else {
            return false;
        };
        let fd_dir = Path::new("/proc").join(pid.to_string()).join("fd");
        let Ok(rd) = fs::read_dir(&fd_dir) else {
            return false;
        };
        for ent in rd.flatten().take(512) {
            let Ok(link) = fs::read_link(ent.path()) else {
                continue;
            };
            let s = link.to_string_lossy();
            if s.starts_with("socket:[")
                || s.starts_with("pipe:[")
                || s.contains("anon_inode:")
            {
                continue;
            }
            let Ok(canon) = link.canonicalize() else {
                continue;
            };
            if canon.starts_with(&prefix) {
                return true;
            }
        }
        false
    }

    /// `true` if `child` is `ancestor` or a descendant process (walk `/proc/.../status` PPid).
    pub fn pid_is_descendant_of(mut pid: u32, ancestor: u32) -> bool {
        if ancestor == 0 {
            return false;
        }
        for _ in 0..96 {
            if pid == ancestor {
                return true;
            }
            if pid == 0 {
                return false;
            }
            let status_path = Path::new("/proc").join(pid.to_string()).join("status");
            let Ok(raw) = fs::read_to_string(&status_path) else {
                return false;
            };
            let Some(ppid) = raw.lines().find_map(|l| {
                l.strip_prefix("PPid:")
                    .map(|s| s.trim().parse::<u32>().ok())
                    .flatten()
            }) else {
                return false;
            };
            pid = ppid;
        }
        false
    }
}

#[cfg(target_os = "linux")]
pub use linux::pid_listens_on_deploy_port;
#[cfg(target_os = "linux")]
pub use linux::{
    listener_pids_for_port, pid_cmdline_or_exe_contains_deploy_root, pid_cwd_starts_with_deploy_root,
    pid_has_open_file_under_deploy_root, pid_is_descendant_of,
};

#[cfg(not(target_os = "linux"))]
pub fn pid_listens_on_deploy_port(_pid: u32, _port: u16) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn listener_pids_for_port(_port: u16) -> Vec<u32> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn pid_cwd_starts_with_deploy_root(_pid: u32, _deploy_root: &std::path::Path) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn pid_cmdline_or_exe_contains_deploy_root(_pid: u32, _deploy_root: &std::path::Path) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn pid_has_open_file_under_deploy_root(_pid: u32, _deploy_root: &std::path::Path) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn pid_is_descendant_of(_child: u32, _ancestor: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::proc_tcp::{tcp6_listen_inodes_for_port_from_text, tcp_listen_inodes_for_port_from_text};

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

    #[test]
    fn parses_tcp6_listen_line() {
        let sample = r"sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00:00000000     0        0 9998888
";
        let found = tcp6_listen_inodes_for_port_from_text(sample, 3000);
        assert!(found.contains(&9998888));
    }
}
