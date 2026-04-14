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
    pub process: ProcessSection,
    #[serde(default)]
    pub docker: DockerSection,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub health: HealthSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSection {
    pub name: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxySection {
    #[serde(default = "default_proxy_type")]
    pub r#type: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default)]
    pub domain: String,
}

fn default_proxy_type() -> String {
    "nginx".to_string()
}

fn default_proxy_port() -> u16 {
    3000
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
            },
            process: ProcessSection::default(),
            docker: DockerSection { enabled: true },
            env: default_env_for_runtime(runtime_type),
            health: HealthSection {
                kind: "http".to_string(),
                path: "/".to_string(),
                port: 3000,
                timeout_ms: 5000,
            },
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
            },
            CmdSection {
                cmd: "npm test".to_string(),
            },
            CmdSection {
                cmd: "npm start".to_string(),
            },
        ),
        "python" => (
            CmdSection {
                cmd: "pip install -r requirements.txt".to_string(),
            },
            CmdSection {
                cmd: "python -m pytest".to_string(),
            },
            CmdSection {
                cmd: "python -m uvicorn main:app --host 0.0.0.0 --port 8000".to_string(),
            },
        ),
        "go" => (
            CmdSection {
                cmd: "go mod download && go build -o app .".to_string(),
            },
            CmdSection {
                cmd: "go test ./...".to_string(),
            },
            CmdSection {
                cmd: "./app".to_string(),
            },
        ),
        "java" => (
            CmdSection {
                cmd: "./mvnw -q package -DskipTests || mvn -q package -DskipTests".to_string(),
            },
            CmdSection {
                cmd: "./mvnw -q test || mvn -q test".to_string(),
            },
            CmdSection {
                cmd: "java -jar target/*.jar".to_string(),
            },
        ),
        "php" => (
            CmdSection {
                cmd: "composer install --no-dev --optimize-autoloader".to_string(),
            },
            CmdSection {
                cmd: "composer test || true".to_string(),
            },
            CmdSection {
                cmd: "php -S 0.0.0.0:8000 -t public".to_string(),
            },
        ),
        "rust" => (
            CmdSection {
                cmd: "cargo build --release".to_string(),
            },
            CmdSection {
                cmd: "cargo test".to_string(),
            },
            CmdSection {
                cmd: "./target/release/app".to_string(),
            },
        ),
        _ => (
            CmdSection::default(),
            CmdSection::default(),
            CmdSection {
                cmd: "echo \"set [start].cmd in pirate.toml\"".to_string(),
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
