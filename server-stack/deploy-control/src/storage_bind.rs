//! Pirate storage: discover mount points for bind sources and run `pirate-storage-bind.sh` via sudo.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

const DEFAULT_PREFIXES: &[&str] = &["/mnt", "/media", "/srv"];
const DEFAULT_BIND_STATE: &str = "/var/lib/pirate/storage-binds.json";

#[derive(Debug, Error)]
pub enum StorageBindError {
    #[error("invalid bind script path")]
    InvalidScript,
    #[error("sudo bind failed: {0}")]
    SudoFailed(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageBindMountCandidate {
    pub mount_point: String,
    pub fstype: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avail_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageBindActive {
    pub volume: String,
    pub source: String,
    pub mount_point: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageBindSourcesView {
    pub candidates: Vec<StorageBindMountCandidate>,
    pub active_binds: Vec<StorageBindActive>,
}

#[cfg(any(test, target_os = "linux"))]
fn decode_proc_mount_line(line: &str) -> Option<(String, String, String)> {
    // `/proc/mounts`: spaces inside device/mount are encoded as `\040`; fields are separated by ASCII spaces.
    let parts: Vec<&str> = line.split_ascii_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    let n = parts.len();
    let fstype = parts.get(n.checked_sub(4)?)?;
    let spec_raw = parts.first()?;
    let mount_raw = parts.get(1)?;
    let spec = spec_raw
        .replace("\\040", " ")
        .replace("\\011", "\t");
    let mount = mount_raw
        .replace("\\040", " ")
        .replace("\\011", "\t");
    Some((spec, mount, (*fstype).to_string()))
}

#[cfg(target_os = "linux")]
fn canonical_parent(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(target_os = "linux")]
fn under_prefixes(mount: &str, prefixes: &[String]) -> bool {
    prefixes
        .iter()
        .any(|p| mount == p.as_str() || mount.starts_with(&format!("{}/", p.trim_end_matches('/'))))
}

#[cfg(target_os = "linux")]
fn statvfs_bytes(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut buf) };
    if rc != 0 {
        return None;
    }
    let fr = buf.f_frsize as u128;
    let blocks = buf.f_blocks as u128;
    let bavail = buf.f_bavail as u128;
    let total = (fr * blocks).min(u64::MAX as u128) as u64;
    let avail = (fr * bavail).min(u64::MAX as u128) as u64;
    Some((avail, total))
}

fn default_prefix_list() -> Vec<String> {
    DEFAULT_PREFIXES.iter().map(|s| (*s).to_string()).collect()
}

/// Parse `PIRATE_STORAGE_BIND_SOURCE_PREFIXES` (`:`-separated) or use `/mnt`, `/media`, `/srv`.
pub fn parse_bind_source_prefixes(raw: Option<&str>) -> Vec<String> {
    let Some(t) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return default_prefix_list();
    };
    t.split(':')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
}

/// List mount points from `/proc/mounts` suitable as bind sources (Linux only).
#[cfg(not(target_os = "linux"))]
pub fn list_storage_bind_mount_candidates(
    _storage_root: &Path,
    _source_prefixes: &[String],
) -> Result<Vec<StorageBindMountCandidate>, StorageBindError> {
    Ok(vec![])
}

/// List mount points from `/proc/mounts` suitable as bind sources (Linux only).
#[cfg(target_os = "linux")]
pub fn list_storage_bind_mount_candidates(
    storage_root: &Path,
    source_prefixes: &[String],
) -> Result<Vec<StorageBindMountCandidate>, StorageBindError> {
    const SKIP: &[&str] = &[
        "proc", "sysfs", "devtmpfs", "tmpfs", "cgroup2", "cgroup", "fusectl", "bpf", "tracefs",
        "securityfs", "mqueue", "configfs", "debugfs", "rpc_pipefs", "binfmt_misc", "hugetlbfs",
        "pstore", "autofs", "overlay", "squashfs", "nsfs", "ramfs",
    ];
    let root_canon = canonical_parent(storage_root);
    let raw = fs::read_to_string("/proc/mounts")?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let Some((spec, mount, fstype)) = decode_proc_mount_line(line) else {
            continue;
        };
        if SKIP.iter().any(|&t| t == fstype.as_str()) {
            continue;
        }
        if !under_prefixes(&mount, source_prefixes) {
            continue;
        }
        let mp = Path::new(&mount);
        let Ok(mp_canon) = mp.canonicalize() else {
            continue;
        };
        if mp_canon == root_canon || mp_canon.starts_with(&root_canon) {
            continue;
        }
        let (avail_bytes, total_bytes) = statvfs_bytes(&mp_canon)
            .map(|(a, t)| (Some(a), Some(t)))
            .unwrap_or((None, None));
        out.push(StorageBindMountCandidate {
            mount_point: mount,
            fstype,
            source: spec,
            avail_bytes,
            total_bytes,
        });
    }
    out.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    out.dedup_by(|a, b| a.mount_point == b.mount_point);
    Ok(out)
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RegistryFile {
    #[serde(default)]
    binds: Vec<RegistryBind>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RegistryBind {
    volume: String,
    source: String,
}

/// Read `/var/lib/pirate/storage-binds.json` (written by `pirate-storage-bind.sh`).
pub fn list_storage_active_binds(
    storage_root: &Path,
    state_path: &Path,
) -> Result<Vec<StorageBindActive>, StorageBindError> {
    if !state_path.is_file() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(state_path)?;
    let j: RegistryFile = serde_json::from_str(&raw).unwrap_or(RegistryFile { binds: vec![] });
    let root = storage_root.to_path_buf();
    let mut out = Vec::new();
    for b in j.binds {
        let mount_point = root
            .join("volumes")
            .join(&b.volume)
            .to_string_lossy()
            .to_string();
        out.push(StorageBindActive {
            volume: b.volume,
            source: b.source,
            mount_point,
        });
    }
    out.sort_by(|a, b| a.volume.cmp(&b.volume));
    Ok(out)
}

pub fn storage_bind_sources_view(
    storage_root: &Path,
    source_prefixes: &[String],
    state_path: &Path,
) -> Result<StorageBindSourcesView, StorageBindError> {
    let candidates = list_storage_bind_mount_candidates(storage_root, source_prefixes)?;
    let active_binds = list_storage_active_binds(storage_root, state_path)?;
    Ok(StorageBindSourcesView {
        candidates,
        active_binds,
    })
}

fn output_text(out: &std::process::Output) -> String {
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

/// `sudo -n /usr/local/lib/pirate/pirate-storage-bind.sh bind <source> <volume>`.
pub fn storage_bind_via_sudo(
    script: &Path,
    source_abs: &str,
    volume_name: &str,
) -> Result<String, StorageBindError> {
    if !script.is_file() {
        return Err(StorageBindError::InvalidScript);
    }
    let script_s = script
        .to_str()
        .ok_or(StorageBindError::InvalidScript)?;
    let out = Command::new("sudo")
        .args(["-n", script_s, "bind", source_abs, volume_name])
        .output()
        .map_err(|e| StorageBindError::SudoFailed(format!("sudo: {e}")))?;
    let txt = output_text(&out);
    if !out.status.success() {
        return Err(StorageBindError::SudoFailed(txt.trim().to_string()));
    }
    Ok(txt.trim().to_string())
}

/// `sudo -n … unbind <volume>`.
pub fn storage_unbind_via_sudo(script: &Path, volume_name: &str) -> Result<String, StorageBindError> {
    if !script.is_file() {
        return Err(StorageBindError::InvalidScript);
    }
    let script_s = script
        .to_str()
        .ok_or(StorageBindError::InvalidScript)?;
    let out = Command::new("sudo")
        .args(["-n", script_s, "unbind", volume_name])
        .output()
        .map_err(|e| StorageBindError::SudoFailed(format!("sudo: {e}")))?;
    let txt = output_text(&out);
    if !out.status.success() {
        return Err(StorageBindError::SudoFailed(txt.trim().to_string()));
    }
    Ok(txt.trim().to_string())
}

pub fn default_storage_bind_state_path() -> PathBuf {
    PathBuf::from(DEFAULT_BIND_STATE)
}

/// Same rules as `pirate-storage-bind.sh` (`volume_name` segment under `volumes/`).
pub fn storage_bind_volume_name_ok(name: &str) -> bool {
    let t = name.trim();
    if t.is_empty() || t.len() > 63 {
        return false;
    }
    let mut it = t.chars();
    let Some(c0) = it.next() else {
        return false;
    };
    if !c0.is_ascii_alphanumeric() {
        return false;
    }
    it.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_proc_mounts_line_simple() {
        let line = r"/dev/sda1 /mnt/data ext4 rw,relatime 0 0";
        let (s, m, t) = decode_proc_mount_line(line).unwrap();
        assert_eq!(s, "/dev/sda1");
        assert_eq!(m, "/mnt/data");
        assert_eq!(t, "ext4");
    }

    #[test]
    fn decode_proc_mounts_line_escaped_space() {
        let line = r"/dev/foo\040bar /mnt/x\040y ext4 rw 0 0";
        let (s, m, t) = decode_proc_mount_line(line).unwrap();
        assert_eq!(s, "/dev/foo bar");
        assert_eq!(m, "/mnt/x y");
        assert_eq!(t, "ext4");
    }

    #[test]
    fn parse_prefixes_colon() {
        let v = parse_bind_source_prefixes(Some("/a:/b"));
        assert_eq!(v, vec!["/a", "/b"]);
    }
}
