//! Nginx upstream routing and release snippet text — shared with deploy-control full `server {}` generator.

use crate::pirate_project::{network_access_server_names, PirateManifest};
use std::collections::BTreeMap;

/// `server_name` line value: space-separated hosts from manifest network/proxy domain fields.
pub fn nginx_server_name_line(manifest: &PirateManifest) -> String {
    let names = network_access_server_names(&manifest.network.access);
    if !names.is_empty() {
        return names.join(" ");
    }
    let d = manifest.proxy.domain.trim();
    if d.is_empty() {
        "_".to_string()
    } else {
        d.to_string()
    }
}

/// True when `path` should get WebSocket upgrade proxy headers.
pub fn nginx_route_websocket(manifest: &PirateManifest, path: &str) -> bool {
    let p = path.trim();
    manifest
        .proxy
        .websocket_paths
        .iter()
        .any(|w| w.trim() == p)
}

/// Nginx `location` block body for reverse-proxy to `upstream` (`host:port`).
pub fn nginx_proxy_location_block(path: &str, upstream: &str, websocket: bool, prefix: &str) -> String {
    let ws = if websocket {
        r#"        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
"#
    } else {
        ""
    };
    format!(
        r#"    location {path} {{
{prefix}{ws}        proxy_pass http://{upstream};
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }}
"#,
        path = path,
        prefix = prefix,
        ws = ws,
        upstream = upstream,
    )
}

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
    let server_name = nginx_server_name_line(manifest);
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
        let ws = nginx_route_websocket(manifest, &path);
        out.push_str(&nginx_proxy_location_block(&path, &target, ws, &lim));
        out.push('\n');
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
    fn nginx_server_name_line_joins_primary_and_extra_domains() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"

[network.access]
domain = "app.example.com"
domains = ["api.example.com", "ws.example.com"]
"#,
        );
        assert_eq!(
            nginx_server_name_line(&m),
            "app.example.com api.example.com ws.example.com"
        );
    }

    #[test]
    fn websocket_location_includes_upgrade_headers() {
        let m = manifest_with_proxy_toml(
            r#"
[project]
name = "x"
version = "1"

[proxy]
type = "nginx"
websocket_paths = ["/api", "/ws"]

[proxy.routes]
"/api" = "127.0.0.1:8080"
"/" = "127.0.0.1:3000"
"#,
        );
        let s = nginx_release_snippet_content(&m).expect("content");
        assert!(s.contains("location /api"));
        assert!(s.contains("location /"));
        let api_block = s.split("location /api {").nth(1).expect("api block");
        assert!(api_block.contains("proxy_set_header Upgrade $http_upgrade"));
        let root_block = s.split("location / {").nth(1).expect("root block");
        let root_only = root_block.split("location /api").next().expect("root only");
        assert!(!root_only.contains("Upgrade $http_upgrade"));
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
