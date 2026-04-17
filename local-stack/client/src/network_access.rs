//! Network & access helpers: service detection, proxy config generation, deploy validation.

use deploy_core::pirate_project::{PirateManifest, ServiceEndpoint};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedService {
    pub name: String,
    pub port: u16,
    pub r#type: String,
    pub source: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDetectionReport {
    pub services: Vec<DetectedService>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeployValidationReport {
    pub allow: bool,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

fn infer_service_name(port: u16) -> &'static str {
    if port == 80 || port == 443 || port == 3000 || port == 5173 {
        "web"
    } else {
        "api"
    }
}

fn parse_ports_from_dockerfile(project_root: &Path) -> Vec<u16> {
    let path = project_root.join("Dockerfile");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let re = Regex::new(r"(?im)^\s*EXPOSE\s+([0-9]+)").ok();
    let mut out = Vec::new();
    if let Some(re) = re {
        for caps in re.captures_iter(&raw) {
            if let Some(m) = caps.get(1) {
                if let Ok(p) = m.as_str().parse::<u16>() {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn parse_ports_from_compose(project_root: &Path) -> Vec<u16> {
    let mut out = Vec::new();
    for name in ["docker-compose.yml", "docker-compose.yaml"] {
        let path = project_root.join(name);
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        let re = Regex::new(r#"(?m)^\s*-\s*"?([0-9]{2,5}):([0-9]{2,5})"?"#).ok();
        if let Some(re) = re {
            for caps in re.captures_iter(&raw) {
                if let Some(m) = caps.get(1) {
                    if let Ok(p) = m.as_str().parse::<u16>() {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

fn parse_ports_from_package_json(project_root: &Path) -> Vec<u16> {
    let path = project_root.join("package.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(p) = v.pointer("/config/port").and_then(|x| x.as_u64()) {
        out.push(p.min(65535) as u16);
    }
    if let Some(scripts) = v.get("scripts").and_then(|x| x.as_object()) {
        let re = Regex::new(r"(?i)(?:--port|PORT=)\s*([0-9]{2,5})").ok();
        if let Some(re) = re {
            for script in scripts.values().filter_map(|x| x.as_str()) {
                if let Some(caps) = re.captures(script) {
                    if let Some(m) = caps.get(1) {
                        if let Ok(p) = m.as_str().parse::<u16>() {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    out
}

fn parse_ports_from_pyproject(project_root: &Path) -> Vec<u16> {
    let path = project_root.join("pyproject.toml");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let re = Regex::new(r"(?i)(?:port\s*=\s*|--port\s+)([0-9]{2,5})").ok();
    let mut out = Vec::new();
    if let Some(re) = re {
        for caps in re.captures_iter(&raw) {
            if let Some(m) = caps.get(1) {
                if let Ok(p) = m.as_str().parse::<u16>() {
                    out.push(p);
                }
            }
        }
    }
    out
}

pub fn detect_services(project_root: &Path, manifest: Option<&PirateManifest>) -> ServiceDetectionReport {
    let mut warnings = Vec::<String>::new();
    let mut by_name = BTreeMap::<String, DetectedService>::new();
    let mut seen_ports = BTreeSet::<u16>::new();

    if let Some(m) = manifest {
        if let Some(ref api) = m.services.api {
            if api.port > 0 {
                by_name.insert(
                    "api".to_string(),
                    DetectedService {
                        name: "api".to_string(),
                        port: api.port,
                        r#type: api.r#type.clone(),
                        source: if api.source.is_empty() {
                            "pirate.toml".to_string()
                        } else {
                            api.source.clone()
                        },
                        confidence: api.confidence,
                    },
                );
                seen_ports.insert(api.port);
            }
        }
        if let Some(ref web) = m.services.web {
            if web.port > 0 {
                by_name.insert(
                    "web".to_string(),
                    DetectedService {
                        name: "web".to_string(),
                        port: web.port,
                        r#type: web.r#type.clone(),
                        source: if web.source.is_empty() {
                            "pirate.toml".to_string()
                        } else {
                            web.source.clone()
                        },
                        confidence: web.confidence,
                    },
                );
                seen_ports.insert(web.port);
            }
        }
    }

    let mut add_detected = |port: u16, source: &str, confidence: f32| {
        if port == 0 || seen_ports.contains(&port) {
            return;
        }
        let name = infer_service_name(port).to_string();
        if by_name.contains_key(&name) {
            warnings.push(format!(
                "multiple candidates for `{name}` (port {port} from {source}); keeping first match"
            ));
            return;
        }
        by_name.insert(
            name.clone(),
            DetectedService {
                name,
                port,
                r#type: "http".to_string(),
                source: source.to_string(),
                confidence,
            },
        );
        seen_ports.insert(port);
    };

    for p in parse_ports_from_dockerfile(project_root) {
        add_detected(p, "Dockerfile:EXPOSE", 0.9);
    }
    for p in parse_ports_from_compose(project_root) {
        add_detected(p, "docker-compose", 0.85);
    }
    for p in parse_ports_from_package_json(project_root) {
        add_detected(p, "package.json", 0.7);
    }
    for p in parse_ports_from_pyproject(project_root) {
        add_detected(p, "pyproject.toml", 0.7);
    }

    ServiceDetectionReport {
        services: by_name.into_values().collect(),
        warnings,
    }
}

pub fn apply_detected_services_to_manifest(
    manifest: &mut PirateManifest,
    report: &ServiceDetectionReport,
) {
    for s in &report.services {
        let endpoint = ServiceEndpoint {
            r#type: s.r#type.clone(),
            port: s.port,
            source: s.source.clone(),
            confidence: s.confidence,
        };
        if s.name == "api" && manifest.services.api.is_none() {
            manifest.services.api = Some(endpoint);
        } else if s.name == "web" && manifest.services.web.is_none() {
            manifest.services.web = Some(endpoint);
        }
    }
}

pub fn generate_proxy_config(manifest: &PirateManifest, server_name: &str) -> Result<String, String> {
    let mut routes = manifest.proxy.routes.clone();
    if routes.is_empty() {
        if let Some(ref web) = manifest.services.web {
            if web.port > 0 {
                routes.insert("/".to_string(), format!("web:{}", web.port));
            }
        }
        if let Some(ref api) = manifest.services.api {
            if api.port > 0 {
                routes.insert("/api".to_string(), format!("api:{}", api.port));
            }
        }
    }
    if routes.is_empty() {
        return Err("no proxy routes found (set [proxy].routes or detect services first)".to_string());
    }
    let mut blocks = String::new();
    for (path, target) in routes {
        let mut split = target.split(':');
        let host = split.next().unwrap_or("");
        let port = split.next().unwrap_or("");
        if host.is_empty() || port.is_empty() {
            return Err(format!("invalid proxy route target `{target}`"));
        }
        blocks.push_str(&format!(
            r#"
    location {} {{
        proxy_pass http://{}:{};
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }}
"#,
            path, host, port
        ));
    }
    Ok(format!(
        r#"server {{
    listen 80;
    server_name {};
{}
}}
"#,
        server_name.trim(),
        blocks
    ))
}

pub fn validate_deploy(manifest: &PirateManifest, occupied_ports: &[u16]) -> DeployValidationReport {
    let mut blockers = Vec::<String>::new();
    let mut warnings = Vec::<String>::new();
    if let Err(e) = manifest.validate_network_proxy() {
        blockers.push(e);
    }

    let mut ports = BTreeSet::<u16>::new();
    if let Some(ref api) = manifest.services.api {
        if api.port > 0 && !ports.insert(api.port) {
            blockers.push(format!("duplicate service port {}", api.port));
        }
    }
    if let Some(ref web) = manifest.services.web {
        if web.port > 0 && !ports.insert(web.port) {
            blockers.push(format!("duplicate service port {}", web.port));
        }
    }
    if manifest.proxy.enabled && manifest.proxy.port > 0 && !ports.insert(manifest.proxy.port) {
        blockers.push(format!(
            "proxy port {} conflicts with service ports",
            manifest.proxy.port
        ));
    }
    for p in occupied_ports {
        if ports.contains(p) {
            blockers.push(format!("port {} is already occupied on host", p));
        }
    }

    let mode = manifest.network.mode.trim().to_ascii_lowercase();
    if mode == "wan" {
        if !manifest.network.tls.https {
            warnings.push(
                "WAN access without HTTPS enabled; enable [network.tls].https and ACME settings"
                    .to_string(),
            );
        }
        if !manifest.network.firewall.managed {
            warnings.push(
                "WAN access with unmanaged firewall; open only required ports explicitly"
                    .to_string(),
            );
        }
    }

    DeployValidationReport {
        allow: blockers.is_empty(),
        blockers,
        warnings,
    }
}
