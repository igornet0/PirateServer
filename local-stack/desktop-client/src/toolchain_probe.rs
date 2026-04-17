//! Local CLI probe for desktop UI (Docker, runtimes, DB clients, …).

use serde::Serialize;
use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

use crate::env_paths::path_for_dev_shell;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainItem {
    pub id: &'static str,
    pub label: String,
    pub installed: bool,
    /// Distinct version lines (`--version` / tool-specific); may list several interpreters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
    pub install_hint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainReport {
    pub items: Vec<ToolchainItem>,
    pub generated_at_ms: u64,
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn command_output_timeout(program: &str, args: &[&str], timeout_ms: u64) -> Option<std::process::Output> {
    let (tx, rx) = std::sync::mpsc::channel();
    let program = program.to_string();
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    std::thread::spawn(move || {
        let mut c = Command::new(&program);
        for a in &args {
            c.arg(a);
        }
        c.env("PATH", path_for_dev_shell());
        let _ = tx.send(c.output());
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(Ok(o)) => Some(o),
        _ => None,
    }
}

fn dedupe_versions_preserve_order(mut lines: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    lines.retain(|s| seen.insert(s.clone()));
    lines
}

fn first_line_version(out: &std::process::Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = if !stdout.trim().is_empty() {
        stdout.into_owned()
    } else {
        stderr.into_owned()
    };
    let line = combined.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    let mut s = line.chars().take(240).collect::<String>();
    if line.len() > 240 {
        s.push('…');
    }
    Some(s)
}

fn probe_simple(id: &'static str, label: &str, program: &str, args: &[&str], hint: String) -> ToolchainItem {
    let out = command_output_timeout(program, args, 2500);
    let (installed, versions) = match &out {
        Some(o) if o.status.success() => {
            let v = first_line_version(o);
            (v.is_some(), v.into_iter().collect())
        }
        Some(o) if !o.stderr.is_empty() || !o.stdout.is_empty() => {
            // nginx -v exits 0 but version in stderr; some tools exit non-zero but print version
            let v = first_line_version(o);
            if v.is_some() {
                (true, v.into_iter().collect())
            } else {
                (false, vec![])
            }
        }
        _ => (false, vec![]),
    };
    ToolchainItem {
        id,
        label: label.to_string(),
        installed,
        versions,
        install_hint: hint,
    }
}

fn version_line_for_program(program: &str, args: &[&str]) -> Option<String> {
    command_output_timeout(program, args, 2500).and_then(|o| first_line_version(&o))
}

/// Probe several binaries with the same args; dedupe identical version lines.
fn probe_multi_simple(
    id: &'static str,
    label: &str,
    programs: &[String],
    args: &[&str],
    hint: String,
) -> ToolchainItem {
    let mut versions = Vec::new();
    for prog in programs {
        if let Some(line) = version_line_for_program(prog.as_str(), args) {
            versions.push(line);
        }
    }
    versions = dedupe_versions_preserve_order(versions);
    let installed = !versions.is_empty();
    ToolchainItem {
        id,
        label: label.to_string(),
        installed,
        versions,
        install_hint: hint,
    }
}

fn probe_python_versions(hint: String) -> ToolchainItem {
    let mut versions = Vec::new();

    const PY_NAMES: &[&str] = &[
        "python3.14",
        "python3.13",
        "python3.12",
        "python3.11",
        "python3.10",
        "python3.9",
        "python3.8",
        "python3",
        "python",
    ];
    for &name in PY_NAMES {
        if let Some(line) = version_line_for_program(name, &["--version"]) {
            versions.push(line);
        }
    }

    // pyenv: one line per installed version (no duplicate --version spam).
    if let Some(out) = command_output_timeout("pyenv", &["versions", "--bare"], 1500) {
        if out.status.success() || !out.stdout.is_empty() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                versions.push(format!("pyenv: {t}"));
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(out) = command_output_timeout("py", &["-0"], 2500) {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            for line in text.lines() {
                let line = line.trim();
                if line.contains("-V:") || line.contains("Python ") {
                    versions.push(format!("py launcher: {line}"));
                }
            }
        }
    }

    versions = dedupe_versions_preserve_order(versions);
    let installed = !versions.is_empty();
    ToolchainItem {
        id: "python",
        label: "Python".into(),
        installed,
        versions,
        install_hint: hint,
    }
}

fn probe_node_versions(hint: String) -> ToolchainItem {
    let mut programs: Vec<String> = vec!["node".into()];

    #[cfg(unix)]
    {
        programs.extend(
            [
                "/opt/homebrew/bin/node",
                "/opt/homebrew/opt/node/bin/node",
                "/opt/homebrew/opt/node@24/bin/node",
                "/opt/homebrew/opt/node@22/bin/node",
                "/opt/homebrew/opt/node@20/bin/node",
                "/opt/homebrew/opt/node@18/bin/node",
                "/usr/local/bin/node",
                "/usr/local/opt/node/bin/node",
            ]
            .iter()
            .map(|s| (*s).to_string()),
        );
    }

    #[cfg(windows)]
    {
        if let Some(out) = Command::new("where.exe")
            .arg("node")
            .env("PATH", path_for_dev_shell())
            .output()
            .ok()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    let p = line.trim();
                    if !p.is_empty() && std::path::Path::new(p).exists() {
                        programs.push(p.to_string());
                    }
                }
            }
        }
        let mut seen = HashSet::new();
        programs.retain(|p| seen.insert(p.clone()));
    }

    probe_multi_simple("node", "Node.js", &programs, &["--version"], hint)
}

fn hint_node() -> String {
    #[cfg(target_os = "macos")]
    {
        "macOS: brew install node или https://nodejs.org/ . nvm: https://github.com/nvm-sh/nvm".into()
    }
    #[cfg(target_os = "windows")]
    {
        "Windows: https://nodejs.org/ или winget install OpenJS.NodeJS.LTS".into()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "https://nodejs.org/ или пакетный менеджер дистрибутива (apt, dnf, …)".into()
    }
}

fn hint_docker() -> String {
    #[cfg(target_os = "macos")]
    {
        "macOS: brew install --cask docker (Docker Desktop). https://docs.docker.com/desktop/setup/install/mac-install/".into()
    }
    #[cfg(target_os = "windows")]
    {
        "Windows: Docker Desktop — https://docs.docker.com/desktop/setup/install/windows-install/".into()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "https://docs.docker.com/engine/install/".into()
    }
}

fn hint_python() -> String {
    #[cfg(target_os = "macos")]
    {
        "Несколько версий: pyenv, Homebrew python@…, официальные установщики. macOS: brew install python@3.12 или https://www.python.org/downloads/".into()
    }
    #[cfg(target_os = "windows")]
    {
        "Несколько версий: py launcher (`py -0`), Store, официальные установщики. https://www.python.org/downloads/ или winget install Python.Python.3.12".into()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Несколько версий: pyenv, пакеты ОС. https://www.python.org/downloads/ или python3 из репозитория".into()
    }
}

fn hint_go() -> String {
    "https://go.dev/dl/ — или brew install go (macOS)".into()
}

fn hint_nginx() -> String {
    #[cfg(target_os = "macos")]
    {
        "macOS: brew install nginx. Для production часто nginx на сервере, не на Mac.".into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Пакет ОС или https://nginx.org/ . В Docker часто используют образ nginx.".into()
    }
}

fn hint_psql() -> String {
    "Клиент PostgreSQL: macOS brew install libpq (psql), Windows — установщик PostgreSQL или https://www.postgresql.org/download/ . Альтернатива: только Docker-образ postgres.".into()
}

fn hint_redis_cli() -> String {
    "macOS: brew install redis. Или используйте redis в Docker (redis:7-alpine).".into()
}

/// Probe common CLI tools; no network; missing binary => `installed: false`.
pub fn probe_local_toolchain() -> ToolchainReport {
    let mut items: Vec<ToolchainItem> = Vec::new();

    items.push(probe_simple(
        "docker",
        "Docker",
        "docker",
        &["--version"],
        hint_docker(),
    ));

    items.push(probe_simple(
        "docker_compose",
        "Docker Compose (plugin)",
        "docker",
        &["compose", "version"],
        "Docker Compose v2 входит в Docker Desktop. Старый бинарник: pip install docker-compose".into(),
    ));

    items.push(probe_simple(
        "docker_compose_v1",
        "docker-compose (standalone)",
        "docker-compose",
        &["--version"],
        "Нужен только если нет `docker compose`. pip install docker-compose или Docker Desktop".into(),
    ));

    items.push(probe_node_versions(hint_node()));

    items.push(probe_simple(
        "npm",
        "npm",
        "npm",
        &["--version"],
        "Устанавливается вместе с Node.js".into(),
    ));

    items.push(probe_python_versions(hint_python()));

    items.push(probe_simple(
        "go",
        "Go",
        "go",
        &["version"],
        hint_go(),
    ));

    // nginx -v writes to stderr, often exit 0
    items.push(probe_simple(
        "nginx",
        "nginx",
        "nginx",
        &["-v"],
        hint_nginx(),
    ));

    let psql = probe_simple(
        "psql",
        "psql (PostgreSQL client)",
        "psql",
        &["--version"],
        hint_psql(),
    );
    if psql.installed {
        items.push(psql);
    } else {
        items.push(probe_simple(
            "pg_config",
            "pg_config",
            "pg_config",
            &["--version"],
            hint_psql(),
        ));
    }

    items.push(probe_simple(
        "redis_cli",
        "redis-cli",
        "redis-cli",
        &["--version"],
        hint_redis_cli(),
    ));

    ToolchainReport {
        items,
        generated_at_ms: now_ms(),
    }
}
