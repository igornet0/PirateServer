//! Long-running local stack: optional `docker-compose.pirate.yml` + `[start].cmd` from `pirate.toml`.

use crate::env_paths::path_for_dev_shell;
use deploy_client::apply_generated_files;
use deploy_core::pirate_project::PirateManifest;
use deploy_core::process_manager;
use parking_lot::Mutex;
use serde::Serialize;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// If `package.json` exists but `node_modules` is missing, run `npm install` once (same shell as start).
fn ensure_node_modules_if_needed(project_root: &Path) -> Result<(), String> {
    if !project_root.join("package.json").is_file() {
        return Ok(());
    }
    if project_root.join("node_modules").is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    let out = {
        Command::new("/bin/bash")
            .arg("-lc")
            .arg("npm install")
            .current_dir(project_root)
            .env("PATH", path_for_dev_shell())
            .output()
            .map_err(|e| format!("npm install: {e}"))?
    };
    #[cfg(windows)]
    let out = {
        Command::new("cmd")
            .args(["/C", "npm install"])
            .current_dir(project_root)
            .env("PATH", path_for_dev_shell())
            .output()
            .map_err(|e| format!("npm install: {e}"))?
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "нужен npm install (нет node_modules): {stderr}{}",
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" stdout: {stdout}")
            }
        ));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDevStatus {
    pub running: bool,
    pub path: Option<String>,
}

/// One line from `[start].cmd` stdout or stderr (for UI or file logging).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalDevStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDevLogLine {
    pub stream: LocalDevStream,
    pub line: String,
}

type LogFn = Arc<dyn Fn(LocalDevLogLine) + Send + Sync>;

fn spawn_line_reader<R: Read + Send + 'static>(
    r: R,
    stream: LocalDevStream,
    on_line: LogFn,
    tail: Option<Arc<Mutex<Vec<String>>>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(r);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(ref t) = tail {
                let mut g = t.lock();
                g.push(line.clone());
                while g.len() > 120 {
                    g.remove(0);
                }
            }
            on_line(LocalDevLogLine { stream, line });
        }
    })
}

fn vec_tail_join(v: &[String]) -> String {
    v.iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

struct State {
    project_root: PathBuf,
    child: Child,
    compose_started: bool,
    reader_joins: Vec<JoinHandle<()>>,
}

fn join_reader_joins(joins: Vec<JoinHandle<()>>) {
    for h in joins {
        let _ = h.join();
    }
}

static GLOBAL: Mutex<Option<State>> = Mutex::new(None);

fn manifest_listen_port(m: &PirateManifest) -> u16 {
    let p = m.health.port;
    if p > 0 {
        p
    } else {
        m.proxy.port
    }
}

/// Fail fast if nothing can bind to the app port (typical: previous local run still listening).
fn assert_port_free(port: u16) -> Result<(), String> {
    use std::io::ErrorKind;
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => {
            drop(l);
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AddrInUse => Err(format!(
            "порт {port} уже занят (часто — не остановлен предыдущий «Запустить локально» или другой dev-сервер). Нажмите «Остановить» или завершите процесс, слушающий этот порт (например: lsof -i :{port})."
        )),
        Err(e) => Err(format!("не удалось проверить порт {port}: {e}")),
    }
}

fn apply_manifest_env(cmd: &mut Command, project_root: &Path, manifest: &PirateManifest) {
    for (k, v) in process_manager::load_dotenv(project_root) {
        cmd.env(k, v);
    }
    for (k, v) in &manifest.env {
        cmd.env(k, v);
    }
    cmd.env("PORT", manifest_listen_port(manifest).to_string());
}

fn docker_compose_up(project_root: &PathBuf) -> Result<(), String> {
    let out = Command::new("docker")
        .args([
            "compose",
            "-f",
            "docker-compose.pirate.yml",
            "up",
            "-d",
        ])
        .current_dir(project_root)
        .output()
        .map_err(|e| format!("docker compose up: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "docker compose up failed: {stderr}{}",
            if stdout.is_empty() {
                String::new()
            } else {
                format!(" stdout: {stdout}")
            }
        ));
    }
    Ok(())
}

fn docker_compose_down(project_root: &PathBuf) {
    let _ = Command::new("docker")
        .args([
            "compose",
            "-f",
            "docker-compose.pirate.yml",
            "down",
            "--remove-orphans",
        ])
        .current_dir(project_root)
        .output();
}

#[cfg(unix)]
fn spawn_start_command(
    start_cmd: &str,
    project_root: &PathBuf,
    manifest: &PirateManifest,
) -> Result<Child, String> {
    use std::os::unix::process::CommandExt;
    // `-l` login shell loads ~/.profile / ~/.bash_profile so nvm/fnm/Homebrew `npm` is on PATH.
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-lc")
        .arg(start_cmd)
        .current_dir(project_root)
        .env("PATH", path_for_dev_shell());
    apply_manifest_env(&mut cmd, project_root, manifest);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| format!("start command: {e}"))
}

#[cfg(windows)]
fn spawn_start_command(
    start_cmd: &str,
    project_root: &PathBuf,
    manifest: &PirateManifest,
) -> Result<Child, String> {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", start_cmd])
        .current_dir(project_root)
        .env("PATH", path_for_dev_shell());
    apply_manifest_env(&mut cmd, project_root, manifest);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start command: {e}"))
}

#[cfg(unix)]
fn kill_child_tree(child: &mut Child) {
    let pid = child.id() as i32;
    unsafe {
        let _ = libc::kill(-pid, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(800));
    if child.try_wait().ok().flatten().is_none() {
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.wait();
    }
}

#[cfg(windows)]
fn kill_child_tree(child: &mut Child) {
    let pid = child.id();
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
    let _ = child.wait();
}

/// Regenerate sidecars, optionally start Docker services, then run `[start].cmd` in the background.
///
/// `on_log` is invoked from background threads for each stdout/stderr line (keeps pipes drained).
pub fn start_local_dev_stack(
    project_root: PathBuf,
    on_log: Option<Arc<dyn Fn(LocalDevLogLine) + Send + Sync>>,
) -> Result<(), String> {
    let p = project_root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", project_root.display()))?;
    if !p.is_dir() {
        return Err("not a directory".into());
    }

    {
        let mut g = GLOBAL.lock();
        if let Some(mut st) = g.take() {
            if st.child.try_wait().ok().flatten().is_none() {
                *g = Some(st);
                return Err(
                    "локальный запуск уже активен — сначала нажмите «Остановить»".into(),
                );
            }
            if st.compose_started {
                docker_compose_down(&st.project_root);
            }
        }
    }

    apply_generated_files(&p)?;

    ensure_node_modules_if_needed(&p)?;

    let m = PirateManifest::read_file(&p.join("pirate.toml"))
        .map_err(|e| format!("pirate.toml: {e}"))?;
    let start_cmd = m.start.cmd.trim();
    let compose_path = p.join("docker-compose.pirate.yml");
    let mut compose_started = false;

    if compose_path.is_file() {
        let meta = std::fs::metadata(&compose_path).map_err(|e| e.to_string())?;
        if meta.len() > 0 {
            docker_compose_up(&p)?;
            compose_started = true;
        }
    }

    if start_cmd.is_empty() {
        if compose_started {
            docker_compose_down(&p);
            return Err(
                "в pirate.toml пустой [start].cmd — нечего запускать после сервисов".into(),
            );
        }
        return Err(
            "нет docker-compose.pirate.yml с сервисами и пустой [start].cmd — укажите команду запуска или включите сервисы в [services] и выполните Apply gen".into(),
        );
    }

    assert_port_free(manifest_listen_port(&m))?;

    let mut child = spawn_start_command(start_cmd, &p, &m)?;

    let emit: LogFn = on_log.unwrap_or_else(|| Arc::new(|_: LocalDevLogLine| {}));

    let tail_stdout = Arc::new(Mutex::new(Vec::<String>::new()));
    let tail_stderr = Arc::new(Mutex::new(Vec::<String>::new()));

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut reader_joins = Vec::new();
    if let Some(s) = stdout {
        reader_joins.push(spawn_line_reader(
            s,
            LocalDevStream::Stdout,
            emit.clone(),
            Some(tail_stdout.clone()),
        ));
    }
    if let Some(s) = stderr {
        reader_joins.push(spawn_line_reader(
            s,
            LocalDevStream::Stderr,
            emit.clone(),
            Some(tail_stderr.clone()),
        ));
    }

    std::thread::sleep(Duration::from_millis(500));
    if child.try_wait().ok().flatten().is_some() {
        join_reader_joins(reader_joins);
        let _ = child.wait();
        let stderr = vec_tail_join(&tail_stderr.lock());
        let stdout = vec_tail_join(&tail_stdout.lock());
        if compose_started {
            docker_compose_down(&p);
        }
        let tail = format!(
            "{}{}",
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(" stderr: {}", stderr.trim())
            },
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" stdout: {}", stdout.trim())
            }
        );
        let mut msg = format!(
            "процесс запуска сразу завершился — проверьте [start].cmd и зависимости (npm install).{tail}"
        );
        if stderr.contains("EADDRINUSE")
            || stderr.contains("address already in use")
            || stderr.contains("уже используется")
        {
            let port = manifest_listen_port(&m);
            msg.push_str(&format!(
                " Порт {port} занят — остановите предыдущий процесс или измените [health].port и [proxy].port в pirate.toml (приложение должно слушать process.env.PORT)."
            ));
        }
        return Err(msg);
    }

    let mut g = GLOBAL.lock();
    *g = Some(State {
        project_root: p,
        child,
        compose_started,
        reader_joins,
    });
    Ok(())
}

pub fn stop_local_dev_stack() -> Result<(), String> {
    let mut g = GLOBAL.lock();
    let Some(mut st) = g.take() else {
        return Err("локальный запуск не активен".into());
    };
    kill_child_tree(&mut st.child);
    join_reader_joins(st.reader_joins);
    if st.compose_started {
        docker_compose_down(&st.project_root);
    }
    Ok(())
}

pub fn local_dev_status() -> LocalDevStatus {
    let mut g = GLOBAL.lock();
    let Some(mut st) = g.take() else {
        return LocalDevStatus {
            running: false,
            path: None,
        };
    };

    match st.child.try_wait() {
        Ok(None) | Err(_) => {
            let path = st.project_root.display().to_string();
            *g = Some(st);
            LocalDevStatus {
                running: true,
                path: Some(path),
            }
        }
        Ok(Some(_)) => {
            join_reader_joins(st.reader_joins);
            if st.compose_started {
                docker_compose_down(&st.project_root);
            }
            LocalDevStatus {
                running: false,
                path: None,
            }
        }
    }
}
