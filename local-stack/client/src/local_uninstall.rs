//! Remove local CLI config and run native stack uninstall (Linux).

use std::path::{Path, PathBuf};

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("pirate-client"))
}

/// Deletes `pirate-client` under the platform config directory (connection, identity, settings).
pub fn remove_local_client_config() -> Result<(), std::io::Error> {
    let Some(dir) = config_dir() else {
        return Ok(());
    };
    remove_local_client_dir(&dir)
}

pub fn remove_local_client_dir(dir: &Path) -> Result<(), std::io::Error> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

const UNINSTALL_SCRIPT: &str = "/usr/local/share/pirate-uninstall/uninstall.sh";

/// Run the installed copy of `uninstall.sh` under `/usr/local/share/pirate-uninstall/` (requires root via `sudo`).
pub fn run_uninstall_stack(
    services_only: bool,
    remove_bundle_dir: bool,
    bundle_dir: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(target_os = "linux") {
        return Err("pirate uninstall stack is only supported on Linux".into());
    }
    if !Path::new(UNINSTALL_SCRIPT).exists() {
        return Err(format!(
            "not found: {UNINSTALL_SCRIPT} (re-run install from a recent bundle, or run sudo ./uninstall.sh from the extracted archive directory)"
        )
        .into());
    }

    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(UNINSTALL_SCRIPT);
    if services_only {
        cmd.arg("--services-only");
    }
    if let Some(p) = bundle_dir {
        let s = p.to_string_lossy();
        if s.is_empty() {
            return Err("--bundle-dir path is empty".into());
        }
        cmd.arg(format!("--remove-bundle-dir={s}"));
    } else if remove_bundle_dir {
        cmd.arg("--remove-bundle-dir");
    }

    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn remove_local_client_dir_deletes_tree() {
        let tmp = std::env::temp_dir().join(format!(
            "pirate-local-uninstall-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("nested")).unwrap();
        fs::write(tmp.join("nested/x"), b"1").unwrap();
        assert!(tmp.exists());
        remove_local_client_dir(&tmp).unwrap();
        assert!(!tmp.exists());
    }
}
