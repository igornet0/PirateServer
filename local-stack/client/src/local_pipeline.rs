//! Local build / test / docker / smoke (`pirate build`, `pirate test`, `pirate test-local`).

use deploy_core::pirate_project::PirateManifest;
use deploy_core::process_manager;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize)]
pub struct StepResult {
    pub step: String,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stderr_tail: String,
}

fn run_shell_cwd(cmd: &str, cwd: &Path) -> Result<std::process::Output, String> {
    if cmd.trim().is_empty() {
        return Err("empty command".to_string());
    }
    #[cfg(unix)]
    let mut c = std::process::Command::new("/bin/sh");
    #[cfg(not(unix))]
    let mut c = std::process::Command::new("sh");
    c.arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c.output().map_err(|e| format!("spawn: {e}"))
}

fn tail(s: &[u8], max: usize) -> String {
    let t = String::from_utf8_lossy(s);
    let t = t.trim();
    if t.len() <= max {
        t.to_string()
    } else {
        t.chars().rev().take(max).collect::<String>().chars().rev().collect()
    }
}

/// Run `[build].cmd` from `pirate.toml`.
pub fn run_build(project_root: &Path) -> Result<StepResult, String> {
    let m = load_manifest(project_root)?;
    let out = run_shell_cwd(&m.build.cmd, project_root)?;
    let ok = out.status.success();
    Ok(StepResult {
        step: "build".to_string(),
        ok,
        exit_code: out.status.code(),
        stderr_tail: tail(&out.stderr, 4000),
    })
}

/// Run `[test].cmd`.
pub fn run_test(project_root: &Path) -> Result<StepResult, String> {
    let m = load_manifest(project_root)?;
    let out = run_shell_cwd(&m.test.cmd, project_root)?;
    let ok = out.status.success();
    Ok(StepResult {
        step: "test".to_string(),
        ok,
        exit_code: out.status.code(),
        stderr_tail: tail(&out.stderr, 4000),
    })
}

fn load_manifest(project_root: &Path) -> Result<PirateManifest, String> {
    let p = project_root.join("pirate.toml");
    PirateManifest::read_file(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Ensure `Dockerfile` exists; generate if missing.
pub fn ensure_dockerfile(project_root: &Path) -> Result<std::path::PathBuf, String> {
    let dockerfile = project_root.join("Dockerfile");
    if dockerfile.is_file() {
        return Ok(dockerfile);
    }
    let m = load_manifest(project_root)?;
    let content = generated_dockerfile(&m)?;
    std::fs::write(&dockerfile, &content).map_err(|e| e.to_string())?;
    Ok(dockerfile)
}

fn generated_dockerfile(m: &PirateManifest) -> Result<String, String> {
    let rt = m.runtime.r#type.as_str();
    let expose = m.proxy.port;
    if rt == "docker" {
        return Err(
            "runtime type docker requires an existing Dockerfile; add one or change [runtime].type"
                .to_string(),
        );
    }
    if !m.runtime.version.is_empty() {
        match rt {
            "node" => {
                return Ok(format!(
                    "FROM node:{}-alpine\nWORKDIR /app\nCOPY . .\nRUN npm install && npm run build || true\nEXPOSE {}\nCMD [\"npm\", \"start\"]\n",
                    m.runtime.version, expose
                ));
            }
            "python" => {
                let esc = m.start.cmd.replace('\"', "\\\"");
                return Ok(format!(
                    "FROM python:{}-slim\nWORKDIR /app\nCOPY . .\nRUN pip install --no-cache-dir -r requirements.txt 2>/dev/null || pip install --no-cache-dir .\nEXPOSE {}\nCMD [\"sh\", \"-c\", \"{}\"]\n",
                    m.runtime.version, expose, esc
                ));
            }
            _ => {}
        }
    }
    let ver = match rt {
        "node" => "18-alpine",
        "python" => "3.11-slim",
        "go" => "1.22-alpine",
        "rust" => "1.76-slim",
        "php" => "8.2-cli",
        "java" => "17-jdk",
        _ => "latest",
    };
    let base = match rt {
        "node" => format!("FROM node:{ver}\nWORKDIR /app\nCOPY . .\nRUN npm install && npm run build || true\n"),
        "python" => format!(
            "FROM python:{ver}\nWORKDIR /app\nCOPY . .\nRUN pip install --no-cache-dir -r requirements.txt 2>/dev/null || pip install --no-cache-dir .\n"
        ),
        "go" => "FROM golang:1.22-alpine AS build\nWORKDIR /src\nCOPY . .\nRUN go build -o /app/bin .\nFROM alpine:3.19\nWORKDIR /app\nCOPY --from=build /app/bin ./app\n"
            .to_string(),
        "rust" => "FROM rust:1.76-slim AS build\nWORKDIR /src\nCOPY . .\nRUN cargo build --release\nFROM debian:bookworm-slim\nWORKDIR /app\nCOPY --from=build /src/target/release/ ./\n"
            .to_string(),
        "php" => format!("FROM php:{ver}\nWORKDIR /app\nCOPY . .\nRUN if [ -f composer.json ]; then curl -sS https://getcomposer.org/installer | php && php composer.phar install --no-dev || true; fi\n"),
        "java" => format!("FROM eclipse-temurin:{ver}\nWORKDIR /app\nCOPY . .\n"),
        _ => format!("FROM alpine:3.19\nWORKDIR /app\nCOPY . .\n"),
    };
    let cmd = match rt {
        "node" => "CMD [\"npm\", \"start\"]\n".to_string(),
        "go" => "CMD [\"./app\"]\n".to_string(),
        "rust" => "CMD [\"sh\", \"-c\", \"exec ./$(ls -A | head -1)\"]\n".to_string(),
        _ => format!(
            "CMD [\"sh\", \"-c\", {}]\n",
            serde_json::to_string(&m.start.cmd).unwrap_or_else(|_| "\"echo noop\"".to_string())
        ),
    };
    Ok(format!("{base}EXPOSE {expose}\n{cmd}"))
}

/// Build Docker image and run container briefly; HTTP check on `health` port/path.
pub fn test_local_docker(project_root: &Path, image_tag: &str) -> Result<StepResult, String> {
    let m = load_manifest(project_root)?;
    ensure_dockerfile(project_root)?;
    let port = m.health.port;
    let path = if m.health.path.is_empty() {
        "/".to_string()
    } else {
        m.health.path.clone()
    };

    let build = std::process::Command::new("docker")
        .args(["build", "-t", image_tag, "."])
        .current_dir(project_root)
        .output()
        .map_err(|e| format!("docker build: {e}"))?;
    if !build.status.success() {
        return Ok(StepResult {
            step: "test-local".to_string(),
            ok: false,
            exit_code: build.status.code(),
            stderr_tail: tail(&build.stderr, 4000),
        });
    }

    let name = format!("pirate-test-{}", std::process::id());
    let mut run = std::process::Command::new("docker");
    run.args([
        "run",
        "-d",
        "--rm",
        "--name",
        &name,
        "-p",
        &format!("{}:{}", port, port),
        image_tag,
    ])
    .current_dir(project_root);
    let up = run.output().map_err(|e| format!("docker run: {e}"))?;
    if !up.status.success() {
        return Ok(StepResult {
            step: "test-local".to_string(),
            ok: false,
            exit_code: up.status.code(),
            stderr_tail: tail(&up.stderr, 4000),
        });
    }

    std::thread::sleep(Duration::from_secs(2));
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let ok_http = process_manager::http_health_check(
        &url,
        Duration::from_millis(m.health.timeout_ms.max(1000)),
    );
    let _ = std::process::Command::new("docker")
        .args(["stop", &name])
        .output();

    Ok(StepResult {
        step: "test-local".to_string(),
        ok: ok_http,
        exit_code: if ok_http { Some(0) } else { Some(1) },
        stderr_tail: if ok_http {
            format!("HTTP OK {url}")
        } else {
            format!("HTTP check failed {url}")
        },
    })
}

/// Write `run.sh` + refresh `docker-compose.pirate.yml` from manifest (for packaging).
pub fn apply_generated_files(project_root: &Path) -> Result<(), String> {
    let m = load_manifest(project_root)?;
    let release_dir = project_root.to_path_buf();
    process_manager::apply_sidecar_manifest(&release_dir, &m).map_err(|e| e.to_string())
}
