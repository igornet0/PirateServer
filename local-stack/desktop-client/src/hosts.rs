//! Optional `hosts` file line for `pirate-client.internal` → 127.0.0.1.
//! Requires write permission (often admin/root); failure is non-fatal.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

pub const HOSTNAME: &str = "pirate-client.internal";

pub fn hosts_file_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// Returns true if the hosts file already maps `HOSTNAME` or we appended the line.
pub fn try_ensure_hosts_mapping() -> bool {
    if std::env::var("PIRATE_SKIP_HOSTS").ok().as_deref() == Some("1") {
        tracing::info!("PIRATE_SKIP_HOSTS=1 — skipping hosts file");
        return false;
    }

    let path = hosts_file_path();
    let mut contents = String::new();
    if let Ok(mut f) = fs::File::open(&path) {
        if f.read_to_string(&mut contents).is_err() {
            return false;
        }
    } else {
        tracing::warn!(path = %path.display(), "cannot read hosts file");
        return false;
    }

    if hosts_has_entry(&contents) {
        tracing::info!("hosts file already contains {}", HOSTNAME);
        return true;
    }

    let line = format!("127.0.0.1 {}\n", HOSTNAME);
    match OpenOptions::new().append(true).open(&path) {
        Ok(mut file) => match file.write_all(line.as_bytes()) {
            Ok(()) => {
                tracing::warn!(
                    path = %path.display(),
                    "appended hosts mapping (may require running as admin once)"
                );
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "cannot write hosts file");
                false
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "cannot open hosts for append");
            false
        }
    }
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
