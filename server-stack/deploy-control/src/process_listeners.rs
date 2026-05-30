//! TCP listener inspection and kill (Linux `/proc`; optional `su` for foreign PIDs).

use crate::types::{KillListenerResultView, ListenerRowView, ProcessListenersView};
use crate::ControlError;
use deploy_core::listen_port_inspect::{
    collect_project_ports, list_all_listener_rows, listener_rows_for_ports, pid_belongs_to_deploy_root,
    ListenerPidRow,
};
use deploy_core::listen_port_owner;
use deploy_core::pirate_project::PirateManifest;
use deploy_core::{
    normalize_project_id, process_manager, project_deploy_root, read_current_version_from_symlink,
    release_dir_for_version, validate_project_id,
};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
fn load_manifest_for_project(deploy_root: &Path, project_id: &str) -> Option<PirateManifest> {
    let root = project_deploy_root(deploy_root, project_id);
    let ver = read_current_version_from_symlink(&root)?;
    let p = release_dir_for_version(&root, &ver).join("pirate.toml");
    PirateManifest::read_file(&p).ok()
}

fn row_from_pid(
    base: ListenerPidRow,
    project_root: &Path,
    project_ports: &BTreeSet<u16>,
    scope: &str,
) -> ListenerRowView {
    let managed = if base.pid > 0 {
        pid_belongs_to_deploy_root(base.pid, project_root)
            || project_ports.contains(&base.port)
    } else {
        false
    };
    ListenerRowView {
        port: base.port,
        protocol: base.protocol,
        bind: base.bind,
        pid: base.pid,
        ppid: base.ppid,
        user: base.user,
        cmdline: base.cmdline,
        scope: scope.to_string(),
        managed_by_project: managed,
    }
}

fn rows_for_project_scope(deploy_root: &Path, project_id: &str) -> Result<Vec<ListenerRowView>, ControlError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (deploy_root, project_id);
        return Err(ControlError::ProcessListeners(
            "TCP listener inspection is only supported on Linux hosts".into(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let root = project_deploy_root(deploy_root, project_id);
        let manifest = load_manifest_for_project(deploy_root, project_id);
        let ports = collect_project_ports(&root, manifest.as_ref());
        let project_ports: BTreeSet<u16> = ports.iter().copied().collect();
        let mut rows = Vec::new();
        if ports.is_empty() {
            return Ok(rows);
        }
        for r in listener_rows_for_ports(&ports) {
            rows.push(row_from_pid(r, &root, &project_ports, "project"));
        }
        Ok(rows)
    }
}

fn rows_for_all_scope(deploy_root: &Path, project_id: &str) -> Result<Vec<ListenerRowView>, ControlError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (deploy_root, project_id);
        return Err(ControlError::ProcessListeners(
            "TCP listener inspection is only supported on Linux hosts".into(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let root = project_deploy_root(deploy_root, project_id);
        let manifest = load_manifest_for_project(deploy_root, project_id);
        let ports = collect_project_ports(&root, manifest.as_ref());
        let project_ports: BTreeSet<u16> = ports.iter().copied().collect();
        let raw = list_all_listener_rows();
        Ok(raw
            .into_iter()
            .map(|r| row_from_pid(r, &root, &project_ports, "all"))
            .collect())
    }
}

pub fn list_process_listeners(
    deploy_root: &Path,
    project_id: &str,
    scope: &str,
) -> Result<ProcessListenersView, ControlError> {
    validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
    let pid = normalize_project_id(project_id);
    let rows = match scope.trim().to_ascii_lowercase().as_str() {
        "all" => rows_for_all_scope(deploy_root, &pid)?,
        "project" | "" => rows_for_project_scope(deploy_root, &pid)?,
        other => {
            return Err(ControlError::ProcessListeners(format!(
                "scope must be 'project' or 'all', got '{other}'"
            )));
        }
    };
    Ok(ProcessListenersView {
        project_id: pid,
        scope: if scope.eq_ignore_ascii_case("all") {
            "all".to_string()
        } else {
            "project".to_string()
        },
        rows,
    })
}

fn pid_allowed_for_kill(
    deploy_root: &Path,
    project_id: &str,
    pid: u32,
    port: Option<u16>,
) -> Result<bool, ControlError> {
    if pid == 0 {
        return Ok(false);
    }
    let root = project_deploy_root(deploy_root, project_id);
    let manifest = load_manifest_for_project(deploy_root, project_id);
    let ports = collect_project_ports(&root, manifest.as_ref());
    if pid_belongs_to_deploy_root(pid, &root) {
        return Ok(true);
    }
    if let Some(port) = port {
        if ports.contains(&port) {
            let listeners = listen_port_owner::listener_pids_for_port(port);
            if listeners.contains(&pid) {
                return Ok(true);
            }
        }
    }
    if let Some(rs) = process_manager::read_runtime_state(&root) {
        if rs.pid == Some(pid) {
            return Ok(true);
        }
        if let Some(pp) = rs.pid {
            if listen_port_owner::pid_is_descendant_of(pid, pp) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn signal_name(sig: &str) -> Result<&'static str, ControlError> {
    match sig.trim().to_ascii_uppercase().as_str() {
        "TERM" | "T" | "15" => Ok("TERM"),
        "KILL" | "K" | "9" => Ok("KILL"),
        other => Err(ControlError::ProcessListeners(format!(
            "signal must be TERM or KILL, got '{other}'"
        ))),
    }
}

fn kill_plain(pid: u32, sig: &str) -> std::io::Result<std::process::Output> {
    Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(pid.to_string())
        .output()
}

fn kill_via_su_root(pid: u32, sig: &str, password: &str) -> Result<std::process::Output, ControlError> {
    if password.is_empty() {
        return Err(ControlError::ElevationFailed("empty root password".into()));
    }
    let script = format!("kill -{sig} {pid}");
    let mut child = Command::new("su")
        .args(["-c", &script, "root"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ControlError::ProcessListeners(format!("su spawn: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(password.as_bytes())
            .map_err(|e| ControlError::ProcessListeners(format!("su stdin: {e}")))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| ControlError::ProcessListeners(format!("su stdin: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| ControlError::ProcessListeners(format!("su wait: {e}")))?;
    Ok(out)
}

pub fn kill_process_listener(
    deploy_root: &Path,
    project_id: &str,
    pid: u32,
    signal: &str,
    port: Option<u16>,
    root_password: Option<&str>,
    allow_foreign: bool,
) -> Result<KillListenerResultView, ControlError> {
    validate_project_id(project_id).map_err(|e| ControlError::Grpc(e.to_string()))?;
    if pid == 0 {
        return Err(ControlError::ProcessListeners("invalid pid".into()));
    }
    let sig = signal_name(signal)?;
    let pid_norm = normalize_project_id(project_id);
    let allowed = pid_allowed_for_kill(deploy_root, &pid_norm, pid, port)?;
    if !allowed && !allow_foreign {
        return Err(ControlError::ProcessListeners(format!(
            "pid {pid} is not attributed to project {pid_norm}; set allow_foreign with root password to force"
        )));
    }

    let plain = kill_plain(pid, sig).map_err(ControlError::Io)?;
    if plain.status.success() {
        return Ok(KillListenerResultView {
            ok: true,
            pid,
            signal: sig.to_string(),
            elevated: false,
            message: format!("sent SIG{sig} to pid {pid}"),
        });
    }

    let stderr = String::from_utf8_lossy(&plain.stderr);
    let needs_root = plain.status.code() == Some(1)
        && (stderr.contains("Operation not permitted")
            || stderr.contains("not permitted")
            || stderr.contains("Permission denied"));

    if !needs_root && !plain.status.success() {
        return Ok(KillListenerResultView {
            ok: false,
            pid,
            signal: sig.to_string(),
            elevated: false,
            message: format!(
                "kill failed ({}): {}",
                plain.status,
                stderr.trim()
            ),
        });
    }

    let Some(pw) = root_password.filter(|s| !s.is_empty()) else {
        return Err(ControlError::ElevationRequired(format!(
            "permission denied killing pid {pid}; provide root_password"
        )));
    };

    if !allowed && !allow_foreign {
        return Err(ControlError::ProcessListeners(
            "foreign pid kill requires allow_foreign=true".into(),
        ));
    }

    let su_out = kill_via_su_root(pid, sig, pw)?;
    if su_out.status.success() {
        return Ok(KillListenerResultView {
            ok: true,
            pid,
            signal: sig.to_string(),
            elevated: true,
            message: format!("sent SIG{sig} to pid {pid} via su"),
        });
    }
    let su_err = String::from_utf8_lossy(&su_out.stderr);
    let su_out_s = String::from_utf8_lossy(&su_out.stdout);
    let merged = format!("{} {}", su_out_s.trim(), su_err.trim());
    if merged.contains("Authentication failure") || merged.contains("incorrect password") {
        return Err(ControlError::ElevationFailed(
            "su authentication failed (wrong password?)".into(),
        ));
    }
    Err(ControlError::ElevationFailed(format!(
        "su kill failed: {}",
        merged.trim()
    )))
}
