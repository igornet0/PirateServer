//! `127.0.0.1:local` → `host:port` TCP forwards and optional `ssh -N -L` sidecars.
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

struct TcpForward {
    stop: Arc<AtomicBool>,
    local_port: u16,
    target: String,
    join: thread::JoinHandle<()>,
}

struct SshTunnel {
    child: Child,
    local_port: u16,
    ssh_target: String,
    remote: String,
}

static TCP: OnceLock<Mutex<HashMap<String, TcpForward>>> = OnceLock::new();
static SSH: OnceLock<Mutex<HashMap<String, SshTunnel>>> = OnceLock::new();

fn tcp_map() -> &'static Mutex<HashMap<String, TcpForward>> {
    TCP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ssh_map() -> &'static Mutex<HashMap<String, SshTunnel>> {
    SSH.get_or_init(|| Mutex::new(HashMap::new()))
}

const LEGACY_ID: &str = "__pirate_default_tcp_forward__";

fn run_tcp_server(
    id: String,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    listener: TcpListener,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(format!("pirate-db-fwd-{}", id.chars().take(12).collect::<String>()))
        .spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((local, _)) => {
                        let r = match TcpStream::connect(addr) {
                            Ok(x) => x,
                            Err(_) => continue,
                        };
                        if let (Ok(mut l1), Ok(mut l2), Ok(mut r1), Ok(mut r2)) = (
                            local.try_clone(),
                            local.try_clone(),
                            r.try_clone(),
                            r.try_clone(),
                        ) {
                            thread::spawn(move || {
                                let _ = io::copy(&mut l1, &mut r2);
                            });
                            thread::spawn(move || {
                                let _ = io::copy(&mut r1, &mut l2);
                            });
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) if stop.load(Ordering::Relaxed) => break,
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            }
        })
        .expect("pirate db forward thread")
}

/// Returns local port. Replaces an existing entry with the same `id`.
pub fn db_tunnel_tcp_start(
    id: String,
    target_host: &str,
    target_port: u16,
) -> Result<u16, String> {
    let addr: SocketAddr = format!("{target_host}:{target_port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let target = format!("{target_host}:{target_port}");
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;
    let local_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let join = run_tcp_server(id.clone(), addr, s2, listener);
    let f = TcpForward {
        stop,
        local_port,
        target,
        join,
    };
    let mut g = tcp_map()
        .lock()
        .map_err(|_| "forward state lock".to_string())?;
    if let Some(old) = g.insert(id, f) {
        old.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(format!("127.0.0.1:{}", old.local_port));
        let _ = old.join.join();
    }
    Ok(local_port)
}

/// Stop one TCP forward by id.
pub fn db_tunnel_tcp_stop(id: &str) -> Result<(), String> {
    let mut g = tcp_map()
        .lock()
        .map_err(|_| "forward state lock".to_string())?;
    if let Some(f) = g.remove(id) {
        f.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(format!("127.0.0.1:{}", f.local_port));
        let _ = f.join.join();
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TcpTunnelView {
    id: String,
    local_port: u16,
    target: String,
    kind: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SshTunnelView {
    id: String,
    local_port: u16,
    remote: String,
    ssh: String,
    kind: &'static str,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

/// Active TCP and SSH port forwards.
pub fn db_tunnel_list_json() -> Result<String, String> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    {
        let g = tcp_map()
            .lock()
            .map_err(|_| "forward state lock".to_string())?;
        for (id, f) in g.iter() {
            out.push(
                serde_json::to_value(TcpTunnelView {
                    id: id.clone(),
                    local_port: f.local_port,
                    target: f.target.clone(),
                    kind: "tcp",
                })
                .map_err(|e| e.to_string())?,
            );
        }
    }
    {
        let mut g = ssh_map()
            .lock()
            .map_err(|_| "forward state lock".to_string())?;
        g.retain(|_id, t| {
            match t.child.try_wait() {
                Ok(None) => true,
                Ok(Some(_)) => false,
                Err(_) => true,
            }
        });
        for (id, t) in g.iter() {
            out.push(
                serde_json::to_value(SshTunnelView {
                    id: id.clone(),
                    local_port: t.local_port,
                    remote: t.remote.clone(),
                    ssh: t.ssh_target.clone(),
                    kind: "ssh",
                    status: "running".to_string(),
                    last_error: None,
                })
                .map_err(|e| e.to_string())?,
            );
        }
    }
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

// --- legacy single-slot (used by DatabasesPanel) ---

/// Returns local port if started; errors if the legacy default slot is already running.
pub fn db_local_forward_start(target_host: &str, target_port: u16) -> Result<u16, String> {
    let g = tcp_map()
        .lock()
        .map_err(|_| "forward state lock".to_string())?;
    if g.contains_key(LEGACY_ID) {
        return Err("A local port forward is already running; stop it first.".into());
    }
    drop(g);
    db_tunnel_tcp_start(LEGACY_ID.into(), target_host, target_port)
}

/// Stops the forward loop; join may take up to ~100ms.
pub fn db_local_forward_stop() -> Result<(), String> {
    db_tunnel_tcp_stop(LEGACY_ID)
}

/// Whether a local forward is thought to be active.
pub fn db_local_forward_local_port() -> Option<u16> {
    let g = tcp_map().lock().ok()?;
    g.get(LEGACY_ID).map(|f| f.local_port)
}

fn ssh_bin() -> &'static str {
    if cfg!(windows) {
        "ssh.exe"
    } else {
        "ssh"
    }
}

/// OpenSSH local forward: `127.0.0.1:local` → `remote_host:remote_port` on the remote side of `ssh_user@ssh_host`.
/// `local_port` 0 = pick a free local port. Requires `ssh` in `PATH`.
pub fn db_tunnel_ssh_start(
    id: String,
    ssh_host: &str,
    ssh_port: u16,
    ssh_user: &str,
    remote_host: &str,
    remote_port: u16,
    local_port: u16,
    identity_path: Option<&str>,
) -> Result<u16, String> {
    let local_port = if local_port == 0 {
        let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let p = l.local_addr().map_err(|e| e.to_string())?.port();
        drop(l);
        p
    } else {
        local_port
    };
    let spec = format!("127.0.0.1:{local_port}:{remote_host}:{remote_port}");
    let ssh_target = format!("{ssh_user}@{ssh_host}");
    let mut cmd = Command::new(ssh_bin());
    cmd.arg("-N")
        .arg("-L")
        .arg(&spec)
        .arg("-p")
        .arg(ssh_port.to_string())
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("-o")
        .arg("ServerAliveInterval=15")
        .arg(&ssh_target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(p) = identity_path {
        if !p.is_empty() {
            cmd.arg("-i").arg(p);
        }
    }
    let child = cmd.spawn().map_err(|e| {
        format!(
            "failed to start ssh (is OpenSSH in PATH?). {e}"
        )
    })?;
    let t = SshTunnel {
        child,
        local_port,
        ssh_target: ssh_target.clone(),
        remote: format!("{remote_host}:{remote_port}"),
    };
    let mut g = ssh_map()
        .lock()
        .map_err(|_| "ssh map lock".to_string())?;
    if let Some(mut old) = g.insert(id, t) {
        let _ = old.child.kill();
        let _ = old.child.wait();
    }
    Ok(local_port)
}

/// Stop a sidecar SSH by id; best-effort `kill` on the child.
pub fn db_tunnel_ssh_stop(id: &str) -> Result<(), String> {
    let mut g = ssh_map()
        .lock()
        .map_err(|_| "ssh map lock".to_string())?;
    if let Some(mut t) = g.remove(id) {
        let _ = t.child.kill();
        let _ = t.child.wait();
    }
    Ok(())
}
