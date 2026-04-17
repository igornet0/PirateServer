//! Maps [`PirateManifest`](crate::pirate_project::PirateManifest) to host inventory ids used by
//! control-api `GET /api/v1/host-services` (must stay aligned with
//! `HOST_SERVICE_IDS` in `deploy-control`).

use crate::pirate_project::PirateManifest;
use std::collections::BTreeSet;

/// Host service ids that can be required from `pirate.toml` (subset of deploy-control whitelist).
pub fn required_host_service_ids(manifest: &PirateManifest) -> Vec<String> {
    let mut set = BTreeSet::<String>::new();

    if manifest.services.postgres {
        set.insert("postgresql".into());
    }
    if manifest.services.redis {
        set.insert("redis".into());
    }
    if manifest.services.mysql {
        set.insert("mysql".into());
    }
    if manifest.services.mongodb {
        set.insert("mongodb".into());
    }

    if let Some(ref srv) = manifest.services.server {
        if !srv.node.trim().is_empty() {
            set.insert("node".into());
        }
    }

    if manifest.proxy.enabled {
        let backend = manifest.proxy.backend.trim();
        if !backend.is_empty() && backend.eq_ignore_ascii_case("nginx") {
            set.insert("nginx".into());
        }
    }

    // Only non-container native runtimes need language stacks on the host; Docker bundles deps.
    let rt = manifest.runtime.r#type.trim().to_ascii_lowercase();
    match rt.as_str() {
        "node" | "nodejs" => {
            set.insert("node".into());
        }
        "python" | "python3" => {
            set.insert("python3".into());
        }
        _ => {}
    }

    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pirate_project::{PirateManifest, ServicesServerSection};

    /// Minimal valid manifest: docker runtime so we do not pull in node/python by default.
    fn minimal_docker() -> PirateManifest {
        let mut m = PirateManifest::default_for_project("x", "docker");
        m.services = Default::default();
        m.proxy.enabled = false;
        m
    }

    #[test]
    fn empty_manifest_no_requirements() {
        let m = minimal_docker();
        assert!(required_host_service_ids(&m).is_empty());
    }

    #[test]
    fn database_flags_map_to_ids() {
        let mut m = minimal_docker();
        m.services.postgres = true;
        m.services.redis = true;
        assert_eq!(
            required_host_service_ids(&m),
            vec!["postgresql".to_string(), "redis".to_string()]
        );
    }

    #[test]
    fn proxy_nginx_enabled_adds_nginx() {
        let mut m = minimal_docker();
        m.proxy.enabled = true;
        m.proxy.backend = "nginx".into();
        assert_eq!(required_host_service_ids(&m), vec!["nginx".to_string()]);
    }

    #[test]
    fn proxy_enabled_non_nginx_backend_skips_nginx() {
        let mut m = minimal_docker();
        m.proxy.enabled = true;
        m.proxy.backend = "caddy".into();
        assert!(required_host_service_ids(&m).is_empty());
    }

    #[test]
    fn runtime_node_adds_node_not_when_docker() {
        let mut m = PirateManifest::default_for_project("x", "node");
        m.services = Default::default();
        m.proxy.enabled = false;
        assert_eq!(required_host_service_ids(&m), vec!["node".to_string()]);

        let d = minimal_docker();
        assert!(required_host_service_ids(&d).is_empty());
    }

    #[test]
    fn runtime_python_adds_python3() {
        let mut m = PirateManifest::default_for_project("x", "python3");
        m.services = Default::default();
        m.proxy.enabled = false;
        assert_eq!(required_host_service_ids(&m), vec!["python3".to_string()]);
    }

    #[test]
    fn services_server_node_requires_node_even_with_docker_runtime() {
        let mut m = minimal_docker();
        m.services.server = Some(ServicesServerSection {
            node: "latest".into(),
        });
        assert_eq!(required_host_service_ids(&m), vec!["node".to_string()]);
    }

    #[test]
    fn sorted_and_deduped() {
        let mut m = minimal_docker();
        m.services.postgres = true;
        m.runtime.r#type = "node".into();
        m.proxy.enabled = true;
        m.proxy.backend = "nginx".into();
        let v = required_host_service_ids(&m);
        assert_eq!(
            v,
            vec![
                "nginx".to_string(),
                "node".to_string(),
                "postgresql".to_string()
            ]
        );
    }
}
