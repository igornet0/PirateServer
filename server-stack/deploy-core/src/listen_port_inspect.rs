//! Enumerate TCP listeners via `/proc` (Linux only).

use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpListenEntry {
    pub port: u16,
    pub bind: String,
    pub inode: u64,
    pub protocol: String,
}

/// Short cmdline for display (NUL-separated args joined with spaces).
pub fn pid_cmdline_short(pid: u32, max_len: usize) -> String {
    let path = format!("/proc/{pid}/cmdline");
    let Ok(raw) = std::fs::read(&path) else {
        return String::new();
    };
    let s: String = raw
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if s.len() <= max_len {
        s
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}

pub fn pid_username(pid: u32) -> String {
    let path = format!("/proc/{pid}/status");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    let uid = raw.lines().find_map(|l| {
        l.strip_prefix("Uid:")
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u32>().ok())
    });
    let Some(uid) = uid else {
        return format!("uid:{pid}");
    };
    if let Ok(pw) = nix_passwd::uid_to_username(uid) {
        return pw;
    }
    format!("uid:{uid}")
}

mod nix_passwd {
    pub fn uid_to_username(uid: u32) -> Result<String, ()> {
        let raw = std::fs::read_to_string("/etc/passwd").map_err(|_| ())?;
        for line in raw.lines() {
            let mut parts = line.split(':');
            let Some(name) = parts.next() else {
                continue;
            };
            let Some(uid_s) = parts.nth(1) else {
                continue;
            };
            if uid_s.parse::<u32>().ok() == Some(uid) {
                return Ok(name.to_string());
            }
        }
        Err(())
    }
}

pub fn pid_ppid(pid: u32) -> Option<u32> {
    let path = format!("/proc/{pid}/status");
    let raw = std::fs::read_to_string(&path).ok()?;
    raw.lines().find_map(|l| {
        l.strip_prefix("PPid:")
            .and_then(|s| s.trim().parse().ok())
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{pid_cmdline_short, pid_ppid, pid_username, TcpListenEntry};
    use crate::listen_port_owner::socket_inodes_for_pid;
    use std::collections::{HashMap, HashSet};
    use std::fs;

    fn hex_port_to_u16(p_hex: &str) -> Option<u16> {
        u16::from_str_radix(p_hex, 16).ok()
    }

    fn ipv4_hex_to_bind(ip_hex: &str) -> String {
        let Some(n) = u32::from_str_radix(ip_hex, 16).ok() else {
            return ip_hex.to_string();
        };
        format!(
            "{}.{}.{}.{}",
            n & 0xFF,
            (n >> 8) & 0xFF,
            (n >> 16) & 0xFF,
            (n >> 24) & 0xFF
        )
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn ipv4_hex_little_endian() {
            assert_eq!(super::ipv4_hex_to_bind("0100007F"), "127.0.0.1");
            assert_eq!(super::ipv4_hex_to_bind("00000000"), "0.0.0.0");
        }
    }

    fn parse_tcp_listen_all(raw: &str, protocol: &str) -> Vec<TcpListenEntry> {
        let mut out = Vec::new();
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
            let Some((addr_part, p_hex)) = local.rsplit_once(':') else {
                continue;
            };
            let Some(port) = hex_port_to_u16(p_hex) else {
                continue;
            };
            let Some(inode) = parts.last().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            let bind = if protocol == "tcp6" {
                if addr_part.chars().all(|c| c == '0') {
                    "[::]".to_string()
                } else {
                    format!("[{addr_part}]")
                }
            } else {
                ipv4_hex_to_bind(addr_part)
            };
            out.push(TcpListenEntry {
                port,
                bind,
                inode,
                protocol: protocol.to_string(),
            });
        }
        out
    }

    pub fn all_tcp_listen_entries() -> Vec<TcpListenEntry> {
        let mut out = Vec::new();
        if let Ok(raw) = fs::read_to_string("/proc/net/tcp") {
            out.extend(parse_tcp_listen_all(&raw, "tcp"));
        }
        if let Ok(raw) = fs::read_to_string("/proc/net/tcp6") {
            out.extend(parse_tcp_listen_all(&raw, "tcp6"));
        }
        out
    }

    fn inode_to_pids() -> HashMap<u64, Vec<u32>> {
        let mut map: HashMap<u64, Vec<u32>> = HashMap::new();
        let Ok(rd) = fs::read_dir("/proc") else {
            return map;
        };
        for ent in rd.flatten() {
            let Ok(pid) = ent.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(inodes) = socket_inodes_for_pid(pid) else {
                continue;
            };
            for ino in inodes {
                map.entry(ino).or_default().push(pid);
            }
        }
        map
    }

    /// Expand listen entries to one row per (port, bind, pid).
    pub fn expand_listen_entries(entries: &[TcpListenEntry]) -> Vec<super::ListenerPidRow> {
        let inode_map = inode_to_pids();
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for e in entries {
            let pids = inode_map.get(&e.inode).cloned().unwrap_or_default();
            if pids.is_empty() {
                let key = (e.port, e.bind.clone(), 0u32);
                if seen.insert(key) {
                    rows.push(super::ListenerPidRow {
                        port: e.port,
                        protocol: e.protocol.clone(),
                        bind: e.bind.clone(),
                        pid: 0,
                        ppid: None,
                        user: String::new(),
                        cmdline: String::new(),
                    });
                }
                continue;
            }
            for pid in pids {
                let key = (e.port, e.bind.clone(), pid);
                if !seen.insert(key) {
                    continue;
                }
                rows.push(super::ListenerPidRow {
                    port: e.port,
                    protocol: e.protocol.clone(),
                    bind: e.bind.clone(),
                    pid,
                    ppid: pid_ppid(pid),
                    user: pid_username(pid),
                    cmdline: pid_cmdline_short(pid, 200),
                });
            }
        }
        rows.sort_by(|a, b| a.port.cmp(&b.port).then(a.pid.cmp(&b.pid)));
        rows
    }
}

#[derive(Debug, Clone)]
pub struct ListenerPidRow {
    pub port: u16,
    pub protocol: String,
    pub bind: String,
    pub pid: u32,
    pub ppid: Option<u32>,
    pub user: String,
    pub cmdline: String,
}

#[cfg(target_os = "linux")]
pub fn list_all_listener_rows() -> Vec<ListenerPidRow> {
    let entries = linux::all_tcp_listen_entries();
    linux::expand_listen_entries(&entries)
}

#[cfg(not(target_os = "linux"))]
pub fn list_all_listener_rows() -> Vec<ListenerPidRow> {
    Vec::new()
}

/// Ports used by a project: runtime_state.port + manifest services/proxy/health.
pub fn collect_project_ports(
    project_root: &Path,
    manifest: Option<&crate::pirate_project::PirateManifest>,
) -> Vec<u16> {
    let mut ports = BTreeSet::<u16>::new();
    if let Some(rs) = crate::process_manager::read_runtime_state(project_root) {
        if rs.port > 0 {
            ports.insert(rs.port);
        }
    }
    if let Some(m) = manifest {
        if let Some(ref api) = m.services.api {
            if api.port > 0 {
                ports.insert(api.port);
            }
        }
        if let Some(ref web) = m.services.web {
            if web.port > 0 {
                ports.insert(web.port);
            }
        }
        if m.proxy.enabled && m.proxy.port > 0 {
            ports.insert(m.proxy.port);
        }
        if m.health.port > 0 {
            ports.insert(m.health.port);
        }
    }
    ports.into_iter().collect()
}

#[cfg(target_os = "linux")]
pub fn listener_rows_for_ports(ports: &[u16]) -> Vec<ListenerPidRow> {
    let all = list_all_listener_rows();
    let want: BTreeSet<u16> = ports.iter().copied().collect();
    all.into_iter()
        .filter(|r| want.contains(&r.port))
        .collect()
}

#[cfg(not(target_os = "linux"))]
pub fn listener_rows_for_ports(_ports: &[u16]) -> Vec<ListenerPidRow> {
    Vec::new()
}

pub fn pid_belongs_to_deploy_root(pid: u32, deploy_root: &Path) -> bool {
    crate::listen_port_owner::pid_cwd_starts_with_deploy_root(pid, deploy_root)
        || crate::listen_port_owner::pid_cmdline_or_exe_contains_deploy_root(pid, deploy_root)
        || crate::listen_port_owner::pid_has_open_file_under_deploy_root(pid, deploy_root)
}
