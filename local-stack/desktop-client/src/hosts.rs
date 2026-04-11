//! Read-only helpers for `pirate-client.internal` in the system hosts file.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

pub const HOSTNAME: &str = "pirate-client.internal";

pub fn hosts_file_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// Returns true if the hosts file maps `HOSTNAME` to an address (read-only check).
pub fn hosts_mapping_present() -> bool {
    let path = hosts_file_path();
    let mut contents = String::new();
    if fs::File::open(&path)
        .and_then(|mut f| f.read_to_string(&mut contents))
        .is_err()
    {
        return false;
    }
    hosts_has_entry(&contents)
}

fn hosts_has_entry(contents: &str) -> bool {
    contents.lines().any(|l| {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            return false;
        }
        t.split_whitespace()
            .any(|part| part.eq_ignore_ascii_case(HOSTNAME))
    })
}
