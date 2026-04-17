//! Nginx upstream routing and release snippet text — shared with deploy-control full `server {}` generator.

use crate::pirate_project::PirateManifest;
use std::collections::BTreeMap;

/// Resolved proxy kind for edge routing: `[proxy].type`, else non-empty `[proxy].backend`, else `nginx`.
pub fn effective_proxy_type(manifest: &PirateManifest) -> String {
    let t = manifest.proxy.r#type.trim();
    if !t.is_empty() {
        return t.to_ascii_lowercase();
    }
    let b = manifest.proxy.backend.trim();
    if !b.is_empty() {
        return b.to_ascii_lowercase();
    }
    "nginx".to_string()
}

/// True when the manifest targets nginx as the reverse proxy for snippet purposes.
/// If `proxy.enabled` is set and `backend` is a non-empty value other than `nginx`, returns false.
pub fn nginx_edge_intended(manifest: &PirateManifest) -> bool {
    if effective_proxy_type(manifest) != "nginx" {
        return false;
    }
    if manifest.proxy.enabled {
        let b = manifest.proxy.backend.trim();
        if !b.is_empty() && !b.eq_ignore_ascii_case("nginx") {
            return false;
        }
    }
    true
}

/// Same route resolution as control-plane `generate_nginx_server_config`, plus fallbacks to
/// `proxy.port` and `health.port` for a single `location /` when services are absent.
pub fn resolve_nginx_upstream_routes(manifest: &PirateManifest) -> BTreeMap<String, String> {
    let mut routes = manifest.proxy.routes.clone();
    if !routes.is_empty() {
        return routes;
    }
    if let Some(ref web) = manifest.services.web {
        if web.port > 0 {
            routes.insert("/".to_string(), format!("127.0.0.1:{}", web.port));
        }
    }
    if let Some(ref api) = manifest.services.api {
        if api.port > 0 {
            routes.insert("/api".to_string(), format!("127.0.0.1:{}", api.port));
        }
    }
    if routes.is_empty() {
        if manifest.proxy.port > 0 {
            routes.insert(
                "/".to_string(),
                format!("127.0.0.1:{}", manifest.proxy.port),
            );
        } else if manifest.health.port > 0 {
            routes.insert(
                "/".to_string(),
                format!("127.0.0.1:{}", manifest.health.port),
            );
        }
    }
    routes
}

/// Why a release snippet was not written (for telemetry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NginxReleaseSnippetSkip {
    NotNginxEdge,
    NoUpstreamRoutes,
}

impl NginxReleaseSnippetSkip {
    pub const fn reason_code(&self) -> &'static str {
        match self {
            Self::NotNginxEdge => "not_nginx_edge",
            Self::NoUpstreamRoutes => "no_upstream_routes",
        }
    }

    pub const fn hint_en(&self) -> &'static str {
        match self {
            Self::NotNginxEdge => {
                "Manifest does not target nginx as the edge proxy (or a non-nginx backend is set while proxy is enabled)."
            }
            Self::NoUpstreamRoutes => {
                "No upstream routes: add [proxy].routes or [services].web/api, or set [proxy].port / [health].port."
            }
        }
    }
}

pub fn nginx_release_skip(manifest: &PirateManifest) -> Option<NginxReleaseSnippetSkip> {
    if !nginx_edge_intended(manifest) {
        return Some(NginxReleaseSnippetSkip::NotNginxEdge);
    }
    if resolve_nginx_upstream_routes(manifest).is_empty() {
        return Some(NginxReleaseSnippetSkip::NoUpstreamRoutes);
    }
    None
}

pub fn should_write_nginx_release_snippet(manifest: &PirateManifest) -> bool {
    nginx_release_skip(manifest).is_none()
}

/// Normalized slug for `limit_req_zone zone=pirate_rl_prj_<slug>` (must match server `projects/<id>.json`).
pub fn antiddos_zone_slug(project_id: &str) -> String {
    let t: String = project_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let t = t.trim_matches('_');
    let mut s = if t.is_empty() {
        "proj".to_string()
    } else {
        t.to_string()
    };
    if s.len() > 64 {
        s.truncate(64);
    }
    s
}

fn antiddos_location_prefix(manifest: &PirateManifest) -> String {
    if !manifest.antiddos.enabled {
        return String::new();
    }
    let slug = antiddos_zone_slug(&manifest.project.deploy_target_project_id());
    let mut burst = manifest.antiddos.burst;
    let mut mconn = manifest.antiddos.max_connections_per_ip;
    if manifest.antiddos.aggressive {
        burst = burst.saturating_mul(7).saturating_div(10).max(1);
        mconn = mconn.saturating_mul(7).saturating_div(10).max(1);
    }
    burst = burst.clamp(1, 1000);
    mconn = mconn.clamp(1, 10000);
    format!(
        "    limit_req zone=pirate_rl_prj_{slug} burst={burst} nodelay;\n    limit_conn pirate_conn_prj_{slug} {mconn};\n"
    )
}

/// `pirate-nginx-snippet.conf` body (fragment for `server { }`, not a full server block).
pub fn nginx_release_snippet_content(manifest: &PirateManifest) -> Result<String, &'static str> {
    if !nginx_edge_intended(manifest) {
        return Err("not_nginx_edge");
    }
    let routes = resolve_nginx_upstream_routes(manifest);
    if routes.is_empty() {
        return Err("no_upstream_routes");
    }
    let server_name = {
        let d = manifest.proxy.domain.trim();
        if !d.is_empty() {
            d.to_string()
        } else if !manifest.network.access.domain.trim().is_empty() {
            manifest.network.access.domain.trim().to_string()
        } else {
            "_".to_string()
        }
    };
    let mut out = String::new();
    out.push_str(&format!("# server_name {server_name};\n"));
    out.push_str("# Generated by Pirate — merge into your server block\n");
    if manifest.antiddos.enabled {
        let slug = antiddos_zone_slug(&manifest.project.deploy_target_project_id());
        out.push_str(&format!(
            "# [antiddos] zones pirate_rl_prj_{slug} / pirate_conn_prj_{slug} must exist on the host (control-api antiddos project + apply).\n"
        ));
    }
    let lim = antiddos_location_prefix(manifest);
    for (path, target) in routes {
        out.push_str(&format!(
            r#"location {} {{
{}    proxy_pass http://{};
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
}}

"#,
            path, lim, target
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pirate_project::PirateManifest;

    fn manifest_with_proxy_toml(toml: &str) -> PirateManifest {
        PirateManifest::parse(toml).expect("parse manifest")
    }

    #[test]
    fn empty_proxy_type_uses_default_and_writes_snippet_with_services_web() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"

[proxy]
type = ""
port = 3000
enabled = false
backend = ""

[services.web]
type = "http"
port = 3000
source = "x"
confidence = 1.0

[health]
port = 3000
"#,
        );
        assert!(should_write_nginx_release_snippet(&m));
        let s = nginx_release_snippet_content(&m).expect("content");
        assert!(s.contains("location /"));
        assert!(s.contains("127.0.0.1:3000"));
    }

    #[test]
    fn explicit_non_nginx_type_skips_snippet() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"

[proxy]
type = "caddy"
port = 3000

[services.web]
type = "http"
port = 3000
source = "x"
confidence = 1.0
"#,
        );
        assert!(!should_write_nginx_release_snippet(&m));
        assert_eq!(
            nginx_release_skip(&m),
            Some(NginxReleaseSnippetSkip::NotNginxEdge)
        );
    }

    #[test]
    fn enabled_with_non_nginx_backend_skips() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"

[proxy]
type = "nginx"
enabled = true
backend = "traefik"
port = 80

[services.web]
type = "http"
port = 3000
source = "x"
confidence = 1.0
"#,
        );
        assert!(!should_write_nginx_release_snippet(&m));
    }

    #[test]
    fn antiddos_adds_limit_directives() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"
deploy_project_id = "my_app"

[proxy]
type = "nginx"
port = 3000
enabled = false
backend = ""

[antiddos]
enabled = true
burst = 15
max_connections_per_ip = 25

[services.web]
type = "http"
port = 3000
source = "x"
confidence = 1.0

[health]
port = 3000
"#,
        );
        let s = nginx_release_snippet_content(&m).expect("content");
        assert!(s.contains("pirate_rl_prj_my_app"));
        assert!(s.contains("limit_req zone=pirate_rl_prj_my_app"));
        assert!(s.contains("limit_conn pirate_conn_prj_my_app"));
    }

    #[test]
    fn no_routes_and_no_ports_skips() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"

[proxy]
type = "nginx"
port = 0
enabled = false

[health]
port = 0
"#,
        );
        assert_eq!(
            nginx_release_skip(&m),
            Some(NginxReleaseSnippetSkip::NoUpstreamRoutes)
        );
    }
}
