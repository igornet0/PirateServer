//! Auto-fix common `pirate.toml` issues from preflight.

use deploy_core::pirate_project::{
    manifest_public_domain_names, normalize_network_mode, PirateManifest,
};
use std::path::Path;

fn apply_network_mode_public(m: &mut PirateManifest) {
    if m.network.mode.trim().eq_ignore_ascii_case("public") {
        m.network.mode = "wan".to_string();
    }
}

fn apply_network_access_public(m: &mut PirateManifest) {
    if normalize_network_mode(&m.network.mode) == "wan" {
        m.network.access.public = true;
    }
}

fn apply_sync_domain_to_network_access(m: &mut PirateManifest) {
    let proxy_domain = m.proxy.domain.trim();
    if proxy_domain.is_empty() {
        return;
    }
    if m.network.access.domain.trim().is_empty() {
        m.network.access.domain = proxy_domain.to_string();
    }
}

fn apply_static_front_profile(m: &mut PirateManifest) {
    apply_network_mode_public(m);
    apply_network_access_public(m);
    apply_sync_domain_to_network_access(m);
    if m.proxy.domain.trim().is_empty() {
        if let Some(d) = manifest_public_domain_names(m).into_iter().next() {
            m.proxy.domain = d;
        }
    }
}

/// Apply a preflight fix by id; writes `pirate.toml` in `project_dir`.
pub fn apply_manifest_fix(project_dir: &Path, fix_id: &str) -> Result<String, String> {
    let manifest_path = project_dir.join("pirate.toml");
    let mut m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    let summary = match fix_id {
        "network_mode_public" => {
            apply_network_mode_public(&mut m);
            "Set [network].mode = wan".to_string()
        }
        "network_access_public" => {
            apply_network_access_public(&mut m);
            "Set [network.access].public = true".to_string()
        }
        "sync_domain_to_network_access" => {
            apply_sync_domain_to_network_access(&mut m);
            "Synced [network.access].domain from [proxy].domain".to_string()
        }
        "static_front_profile" => {
            apply_static_front_profile(&mut m);
            "Applied static nginx-front public profile (mode, access.public, domain sync)".to_string()
        }
        other => return Err(format!("unknown fix_id `{other}`")),
    };

    if let Err(e) = m.validate_network_proxy() {
        return Err(format!("fix applied but manifest still invalid: {e}"));
    }

    let s = m
        .to_toml_string()
        .map_err(|e| format!("serialize pirate.toml: {e}"))?;
    std::fs::write(&manifest_path, s)
        .map_err(|e| format!("write {}: {e}", manifest_path.display()))?;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn static_front_profile_fixes_ex_conf_shape() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("manifest-fix-{pid}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("pirate.toml"),
            r#"
[project]
name = "x"

[network]
mode = "public"

[network.access]
public = false
domain = ""

[proxy]
type = "nginx-front"
domain = "app.example.com"
nginx_conf_path = "./pirate-nginx-snippet.conf"
"#,
        )
        .unwrap();
        let msg = apply_manifest_fix(&root, "static_front_profile").unwrap();
        assert!(msg.contains("static nginx-front"));
        let m = PirateManifest::read_file(&root.join("pirate.toml")).unwrap();
        assert_eq!(m.network.mode, "wan");
        assert!(m.network.access.public);
        assert_eq!(m.network.access.domain, "app.example.com");
        assert!(is_static_nginx_edge(&m));
        let _ = fs::remove_dir_all(&root);
    }
}
