//! Structured manifest issues for preflight and auto-fix.

use crate::pirate_project::{
    is_static_nginx_edge, manifest_public_domain_names, normalize_network_mode, PirateManifest,
};

#[derive(Debug, Clone)]
pub struct ManifestIssue {
    pub id: &'static str,
    pub field: String,
    pub message: String,
    pub hint: String,
    pub fix_label: String,
    pub auto_fixable: bool,
    pub blocking: bool,
}

pub fn diagnose_manifest(m: &PirateManifest) -> Vec<ManifestIssue> {
    let mut out = Vec::new();

    if let Err(e) = m.validate_network_proxy() {
        out.push(ManifestIssue {
            id: "network_validation",
            field: "[network]".to_string(),
            message: e,
            hint: "Review [network], [network.access], [proxy], and [services].".to_string(),
            fix_label: String::new(),
            auto_fixable: false,
            blocking: true,
        });
    }

    let raw_mode = m.network.mode.trim().to_ascii_lowercase();
    if raw_mode == "public" {
        out.push(ManifestIssue {
            id: "network_mode_public",
            field: "[network].mode".to_string(),
            message: "[network].mode = \"public\" is accepted as wan; canonical value is \"wan\"."
                .to_string(),
            hint: "Replace public with wan in pirate.toml for clarity.".to_string(),
            fix_label: "Set mode = wan".to_string(),
            auto_fixable: true,
            blocking: false,
        });
    }

    let normalized = normalize_network_mode(&m.network.mode);
    if normalized == "wan" && is_static_nginx_edge(m) && !m.network.access.public {
        out.push(ManifestIssue {
            id: "network_access_public",
            field: "[network.access].public".to_string(),
            message: "Static public site: [network.access].public is false.".to_string(),
            hint: "Enable public access flag for WAN/static nginx-front deploy.".to_string(),
            fix_label: "Set access.public = true".to_string(),
            auto_fixable: true,
            blocking: false,
        });
    }

    if normalized == "wan" && is_static_nginx_edge(m) {
        let proxy_domain = m.proxy.domain.trim();
        let access_domain = m.network.access.domain.trim();
        if !proxy_domain.is_empty() && access_domain.is_empty() {
            out.push(ManifestIssue {
                id: "sync_domain_to_network_access",
                field: "[network.access].domain".to_string(),
                message: format!(
                    "Domain \"{proxy_domain}\" is in [proxy].domain but [network.access].domain is empty."
                ),
                hint: "Copy primary domain into [network.access].domain for network wizard and telemetry."
                    .to_string(),
                fix_label: "Sync domain to network.access".to_string(),
                auto_fixable: true,
                blocking: false,
            });
        }
    }

    if normalized == "wan"
        && is_static_nginx_edge(m)
        && (raw_mode == "public" || !m.network.access.public || {
            let proxy_domain = m.proxy.domain.trim();
            !proxy_domain.is_empty() && m.network.access.domain.trim().is_empty()
        })
    {
        out.push(ManifestIssue {
            id: "static_front_profile",
            field: "pirate.toml".to_string(),
            message: "Static nginx-front public profile can be normalized in one step.".to_string(),
            hint: "Sets mode=wan, access.public=true, syncs domain from [proxy].domain.".to_string(),
            fix_label: "Fix static front profile".to_string(),
            auto_fixable: true,
            blocking: false,
        });
    }

    out
}

/// Human-readable network summary when no issues remain.
pub fn network_access_summary(m: &PirateManifest) -> String {
    let normalized = normalize_network_mode(&m.network.mode);
    let mode_label = if m.network.mode.trim().eq_ignore_ascii_case("public") {
        format!("{} (wan)", m.network.mode.trim())
    } else {
        normalized.to_string()
    };
    let domains = manifest_public_domain_names(m);
    let domain_part = if domains.is_empty() {
        "no domain".to_string()
    } else {
        domains.join(", ")
    };
    let profile = if is_static_nginx_edge(m) {
        "static nginx-front"
    } else if m.proxy.enabled {
        "reverse-proxy"
    } else {
        "local/private"
    };
    format!("Mode: {mode_label}; domain: {domain_part}; profile: {profile}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pirate_project::PirateManifest;

    fn static_front_toml(mode: &str, access_public: bool, access_domain: &str) -> String {
        format!(
            r#"
[project]
name = "x"

[network]
mode = "{mode}"

[network.access]
public = {access_public}
domain = "{access_domain}"

[proxy]
type = "nginx-front"
domain = "app.example.com"
nginx_conf_path = "./pirate-nginx-snippet.conf"
"#
        )
    }

    #[test]
    fn diagnose_static_front_public_mode_suggests_fixes() {
        let m = PirateManifest::parse(&static_front_toml("public", false, "")).expect("parse");
        assert!(m.validate_network_proxy().is_ok());
        let issues = diagnose_manifest(&m);
        assert!(issues.iter().any(|i| i.id == "network_mode_public"));
        assert!(issues.iter().any(|i| i.id == "network_access_public"));
        assert!(issues.iter().any(|i| i.id == "static_front_profile"));
    }

    #[test]
    fn diagnose_clean_static_front_wan() {
        let m = PirateManifest::parse(&static_front_toml("wan", true, "app.example.com")).expect("parse");
        let issues = diagnose_manifest(&m);
        assert!(issues.iter().all(|i| !i.blocking));
        assert!(issues.is_empty());
    }
}
