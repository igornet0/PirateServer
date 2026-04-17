//! `pirate.toml` schema and helpers (shared by local-stack CLI and deploy-server).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Top-level project manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PirateManifest {
    pub project: ProjectSection,
    #[serde(default)]
    pub runtime: RuntimeSection,
    #[serde(default)]
    pub build: CmdSection,
    #[serde(default)]
    pub test: CmdSection,
    #[serde(default)]
    pub start: CmdSection,
    #[serde(default)]
    pub services: ServicesSection,
    #[serde(default)]
    pub proxy: ProxySection,
    #[serde(default)]
    pub network: NetworkSection,
    #[serde(default)]
    pub process: ProcessSection,
    #[serde(default)]
    pub docker: DockerSection,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub health: HealthSection,
    /// Optional L7 limits for nginx release snippet (`pirate-nginx-snippet.conf`). Requires matching
    /// `limit_req_zone` on the host — use control-api `PUT /api/v1/antiddos/projects/:project_id` then `POST /api/v1/antiddos/apply`.
    #[serde(default)]
    pub antiddos: AntiddosSection,
}

/// Per-project nginx rate limits (zones must exist in `/etc/nginx/conf.d/99-pirate-antiddos-zones.conf` on the server).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AntiddosSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default = "default_antiddos_rps")]
    pub rate_limit_rps: f64,
    #[serde(default = "default_antiddos_burst")]
    pub burst: u32,
    #[serde(default = "default_antiddos_mconn")]
    pub max_connections_per_ip: u32,
}

fn default_antiddos_rps() -> f64 {
    10.0
}
fn default_antiddos_burst() -> u32 {
    20
}
fn default_antiddos_mconn() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    /// App / manifest version (e.g. semver); compared client-side with server after deploy.
    #[serde(default)]
    pub version: String,
    /// gRPC deploy project id for this tree; empty → `default` (same as deploy form).
    #[serde(default)]
    pub deploy_project_id: String,
}

impl ProjectSection {
    /// Target for `GetStatus` / deploy (normalized).
    pub fn deploy_target_project_id(&self) -> String {
        let s = self.deploy_project_id.trim();
        if s.is_empty() {
            crate::normalize_project_id("")
        } else {
            crate::normalize_project_id(s)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeSection {
    #[serde(default = "default_runtime_type")]
    pub r#type: String,
    #[serde(default)]
    pub version: String,
}

fn default_runtime_type() -> String {
    "node".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CmdSection {
    #[serde(default)]
    pub cmd: String,
    /// Relative file/dir path to package for release (preferred single output).
    #[serde(default)]
    pub output_path: String,
    /// Relative file/dir paths to package for release (optional multiple outputs).
    #[serde(default)]
    pub output_paths: Vec<String>,
}

/// Host-side language/runtime hints under `[services.server]` (not inside the app container).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServicesServerSection {
    /// If non-empty after trim (e.g. `latest`, `20`), host inventory should include `node`.
    #[serde(default)]
    pub node: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServicesSection {
    #[serde(default)]
    pub postgres: bool,
    #[serde(default)]
    pub redis: bool,
    #[serde(default)]
    pub mysql: bool,
    #[serde(default)]
    pub mongodb: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServicesServerSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ServiceEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<ServiceEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    #[serde(default = "default_service_type")]
    pub r#type: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub source: String,
    #[serde(default = "default_service_confidence")]
    pub confidence: f32,
}

impl Default for ServiceEndpoint {
    fn default() -> Self {
        Self {
            r#type: default_service_type(),
            port: 0,
            source: String::new(),
            confidence: default_service_confidence(),
        }
    }
}

fn default_service_type() -> String {
    "http".to_string()
}

fn default_service_confidence() -> f32 {
    1.0
}

fn default_proxy_type() -> String {
    "nginx".to_string()
}

fn default_proxy_port() -> u16 {
    3000
}

fn default_proxy_backend() -> String {
    "nginx".to_string()
}

fn deserialize_nonempty_or_default_proxy_type<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(if s.trim().is_empty() {
        default_proxy_type()
    } else {
        s
    })
}

fn deserialize_nonempty_or_default_proxy_backend<'de, D>(
    deserializer: D,
) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(if s.trim().is_empty() {
        default_proxy_backend()
    } else {
        s
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxySection {
    #[serde(default = "default_proxy_type")]
    #[serde(deserialize_with = "deserialize_nonempty_or_default_proxy_type")]
    pub r#type: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_proxy_backend")]
    #[serde(deserialize_with = "deserialize_nonempty_or_default_proxy_backend")]
    pub backend: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSection {
    #[serde(default = "default_network_mode")]
    pub mode: String,
    #[serde(default)]
    pub access: NetworkAccessSection,
    #[serde(default)]
    pub tls: NetworkTlsSection,
    #[serde(default)]
    pub firewall: NetworkFirewallSection,
}

impl Default for NetworkSection {
    fn default() -> Self {
        Self {
            mode: default_network_mode(),
            access: NetworkAccessSection::default(),
            tls: NetworkTlsSection::default(),
            firewall: NetworkFirewallSection::default(),
        }
    }
}

fn default_network_mode() -> String {
    "private".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkAccessSection {
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTlsSection {
    #[serde(default)]
    pub https: bool,
    #[serde(default = "default_tls_provider")]
    pub provider: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub auto_renew: bool,
}

impl Default for NetworkTlsSection {
    fn default() -> Self {
        Self {
            https: false,
            provider: default_tls_provider(),
            email: String::new(),
            auto_renew: true,
        }
    }
}

fn default_tls_provider() -> String {
    "letsencrypt".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkFirewallSection {
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub open_ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProcessSection {
    #[serde(default = "default_process_manager")]
    pub manager: String,
    #[serde(default = "default_restart_policy")]
    pub restart: String,
    #[serde(default = "default_cpu_limit")]
    pub cpu_limit_percent: u32,
    #[serde(default = "default_mem_limit")]
    pub memory_limit_mb: u32,
}

fn default_process_manager() -> String {
    "internal".to_string()
}

fn default_restart_policy() -> String {
    "on-fail".to_string()
}

fn default_cpu_limit() -> u32 {
    100
}

fn default_mem_limit() -> u32 {
    512
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DockerSection {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HealthSection {
    #[serde(default = "default_health_kind")]
    pub kind: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_health_port")]
    pub port: u16,
    #[serde(default = "default_health_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_health_kind() -> String {
    "http".to_string()
}

fn default_health_port() -> u16 {
    3000
}

fn default_health_timeout_ms() -> u64 {
    5000
}

impl PirateManifest {
    pub fn default_for_project(name: &str, runtime_type: &str) -> Self {
        let (build, test, start) = default_commands_for_runtime(runtime_type);
        Self {
            project: ProjectSection {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                deploy_project_id: String::new(),
            },
            runtime: RuntimeSection {
                r#type: runtime_type.to_string(),
                version: default_runtime_version(runtime_type),
            },
            build,
            test,
            start,
            services: ServicesSection::default(),
            proxy: ProxySection {
                r#type: "nginx".to_string(),
                port: 3000,
                domain: String::new(),
                enabled: false,
                backend: default_proxy_backend(),
                routes: BTreeMap::new(),
            },
            network: NetworkSection::default(),
            process: ProcessSection::default(),
            docker: DockerSection { enabled: true },
            env: default_env_for_runtime(runtime_type),
            health: HealthSection {
                kind: "http".to_string(),
                path: "/".to_string(),
                port: 3000,
                timeout_ms: 5000,
            },
            antiddos: AntiddosSection::default(),
        }
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn read_file(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let raw = std::fs::read_to_string(path)?;
        Ok(Self::parse(&raw)?)
    }

    pub fn validate_network_proxy(&self) -> Result<(), String> {
        let mode = self.network.mode.trim().to_ascii_lowercase();
        if mode != "private" && mode != "lan" && mode != "wan" {
            return Err(format!(
                "invalid [network].mode `{}` (expected private|lan|wan)",
                self.network.mode
            ));
        }
        if let Some(ref api) = self.services.api {
            if api.port == 0 {
                return Err("services.api.port must be > 0".to_string());
            }
        }
        if let Some(ref web) = self.services.web {
            if web.port == 0 {
                return Err("services.web.port must be > 0".to_string());
            }
        }
        if let (Some(api), Some(web)) = (&self.services.api, &self.services.web) {
            if api.port == web.port {
                return Err("services.api.port conflicts with services.web.port".to_string());
            }
        }
        if mode == "wan" && self.network.access.public {
            if self.network.access.domain.trim().is_empty() {
                return Err(
                    "network.access.domain is required when network.mode=wan and public=true"
                        .to_string(),
                );
            }
            if !self.proxy.enabled {
                return Err("proxy.enabled must be true when network.mode=wan".to_string());
            }
        }
        Ok(())
    }

    /// Ordered unique release outputs from `[build].output_path[s]`.
    pub fn release_output_paths(&self) -> Vec<String> {
        let mut out = Vec::<String>::new();
        if !self.build.output_path.trim().is_empty() {
            out.push(self.build.output_path.trim().to_string());
        }
        for p in &self.build.output_paths {
            let t = p.trim();
            if t.is_empty() {
                continue;
            }
            if !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
        out
    }
}

fn default_runtime_version(runtime_type: &str) -> String {
    match runtime_type {
        "node" => "18".to_string(),
        "python" => "3.11".to_string(),
        "go" => "1.22".to_string(),
        "java" => "17".to_string(),
        "php" => "8.2".to_string(),
        "rust" => "1.76".to_string(),
        "docker" => "latest".to_string(),
        _ => String::new(),
    }
}

fn default_commands_for_runtime(rt: &str) -> (CmdSection, CmdSection, CmdSection) {
    match rt {
        "node" => (
            CmdSection {
                cmd: "npm install && npm run build".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "npm test".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "npm start".to_string(),
                ..CmdSection::default()
            },
        ),
        "python" => (
            CmdSection {
                cmd: "pip install -r requirements.txt".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "python -m pytest".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "python -m uvicorn main:app --host 0.0.0.0 --port 8000".to_string(),
                ..CmdSection::default()
            },
        ),
        "go" => (
            CmdSection {
                cmd: "go mod download && go build -o app .".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "go test ./...".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "./app".to_string(),
                ..CmdSection::default()
            },
        ),
        "java" => (
            CmdSection {
                cmd: "./mvnw -q package -DskipTests || mvn -q package -DskipTests".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "./mvnw -q test || mvn -q test".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "java -jar target/*.jar".to_string(),
                ..CmdSection::default()
            },
        ),
        "php" => (
            CmdSection {
                cmd: "composer install --no-dev --optimize-autoloader".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "composer test || true".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "php -S 0.0.0.0:8000 -t public".to_string(),
                ..CmdSection::default()
            },
        ),
        "rust" => (
            CmdSection {
                cmd: "cargo build --release".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "cargo test".to_string(),
                ..CmdSection::default()
            },
            CmdSection {
                cmd: "./target/release/app".to_string(),
                ..CmdSection::default()
            },
        ),
        _ => (
            CmdSection::default(),
            CmdSection::default(),
            CmdSection {
                cmd: "echo \"set [start].cmd in pirate.toml\"".to_string(),
                ..CmdSection::default()
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default_node_manifest() {
        let m = PirateManifest::default_for_project("demo", "node");
        let s = m.to_toml_string().expect("toml");
        let m2 = PirateManifest::parse(&s).expect("parse");
        assert_eq!(m2.project.name, "demo");
        assert_eq!(m2.runtime.r#type, "node");
    }

    #[test]
    fn parse_services_server_node() {
        let m = PirateManifest::parse(
            r#"
[project]
name = "x"

[services.server]
node = "latest"
"#,
        )
        .expect("parse");
        assert_eq!(
            m.services.server.as_ref().map(|s| s.node.as_str()),
            Some("latest")
        );
    }

    #[test]
    fn release_output_paths_merges_single_and_list() {
        let m = PirateManifest::parse(
            r#"
[project]
name = "x"

[build]
cmd = "npm run build"
output_path = "dist"
output_paths = ["dist", "public"]
"#,
        )
        .expect("parse");
        assert_eq!(m.release_output_paths(), vec!["dist", "public"]);
    }

    #[test]
    fn validate_network_proxy_requires_domain_for_public_wan() {
        let m = PirateManifest::parse(
            r#"
[project]
name = "x"
[network]
mode = "wan"
[network.access]
public = true
domain = ""
[proxy]
enabled = true
"#,
        )
        .expect("parse");
        assert!(m.validate_network_proxy().is_err());
    }
}

fn default_env_for_runtime(rt: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    match rt {
        "node" => {
            m.insert("NODE_ENV".to_string(), "production".to_string());
        }
        "python" => {
            m.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());
        }
        _ => {}
    }
    m
}

/// Detect runtime from repository root markers.
pub fn detect_runtime(project_root: &Path) -> &'static str {
    if project_root.join("Dockerfile").is_file() {
        return "docker";
    }
    if project_root.join("package.json").is_file() {
        return "node";
    }
    if project_root.join("go.mod").is_file() {
        return "go";
    }
    if project_root.join("Cargo.toml").is_file() {
        return "rust";
    }
    if project_root.join("composer.json").is_file() {
        return "php";
    }
    if project_root.join("requirements.txt").is_file()
        || project_root.join("pyproject.toml").is_file()
    {
        return "python";
    }
    if project_root.join("pom.xml").is_file() || project_root.join("build.gradle").is_file() {
        return "java";
    }
    "node"
}

/// Guess listen port from common files (best-effort).
pub fn guess_port(project_root: &Path, rt: &str) -> u16 {
    if let Ok(s) = std::fs::read_to_string(project_root.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(p) = v.pointer("/config/port").and_then(|x| x.as_u64()) {
                return p.min(65535) as u16;
            }
        }
    }
    match rt {
        "node" | "rust" | "docker" => 3000,
        "python" | "php" => 8000,
        "go" | "java" => 8080,
        _ => 3000,
    }
}

/// `[project].version` from `pirate.toml` in the active release (`current` → `releases/<ver>`).
pub fn read_pirate_project_version_from_deploy_root(project_root: &Path) -> String {
    let Some(v) = crate::read_current_version_from_symlink(project_root) else {
        return String::new();
    };
    let rd = crate::release_dir_for_version(project_root, &v);
    let p = rd.join("pirate.toml");
    match PirateManifest::read_file(&p) {
        Ok(m) => m.project.version.trim().to_string(),
        Err(_) => String::new(),
    }
}
