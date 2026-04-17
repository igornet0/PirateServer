//! Host software inventory (Node, Python, nginx, DB packages) for dashboard «Services» tab.

use crate::nginx::collect_nginx_status;
use crate::types::{HostServiceActionView, HostServiceRow, HostServicesView, NginxStatusView};
use crate::ControlError;
use std::path::Path;
use std::process::Command;

/// Whitelist for `POST /api/v1/host-services/{id}/…` (must match `pirate-host-service.sh`).
pub const HOST_SERVICE_IDS: &[&str] = &[
    "node",
    "python3",
    "nginx",
    "redis",
    "postgresql",
    "mysql",
    "mongodb",
    "mssql",
    "clickhouse",
    "oracle_client",
    "cifs_utils",
];

pub fn host_service_id_allowed(id: &str) -> bool {
    HOST_SERVICE_IDS.iter().any(|s| *s == id)
}

fn output_text(out: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{stdout}{stderr}")
}

fn has_command(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cmd_stdout_trim(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// First non-empty trimmed line (used for `--version` output that may go to stderr).
fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

fn cmd_stderr_first_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stderr.trim().is_empty() {
        first_nonempty_line(&stdout)
    } else {
        first_nonempty_line(&stderr)
    }
}

fn systemctl_is_active(unit: &str) -> Option<bool> {
    let out = Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(false);
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let s = line.trim();
    Some(s == "active")
}

fn dpkg_installed(pkg: &str) -> bool {
    let out = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", pkg])
        .output()
        .ok();
    match out {
        Some(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).contains("install ok installed")
        }
        _ => false,
    }
}

fn collect_cifs_mounts() -> Vec<String> {
    let out = Command::new("findmnt")
        .args(["-t", "cifs", "-n", "-o", "TARGET"])
        .output()
        .ok();
    match out {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn actions_for(
    _id: &str,
    installed: bool,
    dispatch_present: bool,
    oracle_only: bool,
) -> String {
    if !dispatch_present || oracle_only {
        return "none".to_string();
    }
    if !installed {
        "install".to_string()
    } else {
        "remove".to_string()
    }
}

/// Build host service inventory (read-only; no sudo).
pub fn collect_host_services(
    nginx_site_path: &Path,
    nginx_ensure: &Path,
    nginx_apply: &Path,
    dispatch_script: &Path,
) -> HostServicesView {
    let dispatch_script_present = dispatch_script.is_file();
    let ngx: NginxStatusView =
        collect_nginx_status(nginx_site_path, nginx_ensure, nginx_apply);
    let nginx_running = ngx
        .systemd_active
        .as_deref()
        .map(|s| s == "active");

    let node_v = if has_command("node") {
        cmd_stdout_trim("node", &["-v"])
    } else {
        None
    };
    let py_v = cmd_stdout_trim("python3", &["--version"]);

    let redis_inst = has_command("redis-server") || dpkg_installed("redis-server");
    let redis_v = cmd_stdout_trim("redis-server", &["--version"]);
    let redis_run = systemctl_is_active("redis-server");

    let pg_inst = has_command("psql") || dpkg_installed("postgresql");
    let pg_v = cmd_stdout_trim("psql", &["--version"]);
    let pg_run = systemctl_is_active("postgresql");

    let mysql_inst = has_command("mysql") || dpkg_installed("mysql-server");
    let mysql_v = cmd_stdout_trim("mysql", &["--version"]);
    let mysql_run = systemctl_is_active("mysql");

    let mongo_inst = has_command("mongod") || dpkg_installed("mongodb-org");
    let mongo_v = cmd_stdout_trim("mongod", &["--version"]);
    let mongo_run = systemctl_is_active("mongod");

    let mssql_inst = dpkg_installed("mssql-server") || has_command("sqlcmd");
    let mssql_v = if dpkg_installed("mssql-server") {
        cmd_stdout_trim("dpkg-query", &["-W", "-f=${Version}", "mssql-server"])
    } else {
        None
    };
    let mssql_run = systemctl_is_active("mssql-server");
    let mut mssql_notes: Option<String> = None;
    if mssql_inst {
        let setup_done = Path::new("/var/opt/mssql/mssql.conf").is_file();
        if !setup_done || mssql_run == Some(false) {
            mssql_notes = Some(
                "If the service is inactive, run /opt/mssql/bin/mssql-conf setup as root (EULA, SA password)."
                    .to_string(),
            );
        }
    }

    let ch_inst = has_command("clickhouse-client") || dpkg_installed("clickhouse-server");
    let ch_v = if has_command("clickhouse-client") {
        cmd_stderr_first_line("clickhouse-client", &["--version"])
            .or_else(|| cmd_stdout_trim("clickhouse-client", &["--version"]))
    } else {
        None
    };
    let ch_run = systemctl_is_active("clickhouse-server");

    let cifs_inst = has_command("mount.cifs") || dpkg_installed("cifs-utils");
    let cifs_v = if dpkg_installed("cifs-utils") {
        cmd_stdout_trim("dpkg-query", &["-W", "-f=${Version}", "cifs-utils"])
    } else {
        None
    };

    let oracle_notes = Some(
        "Oracle Database is not installed via this stack; use Oracle XE, container images, or Instant Client. See install-oracle-notes.sh."
            .to_string(),
    );

    let mut services = vec![
        HostServiceRow {
            id: "node".to_string(),
            display_name: "Node.js".to_string(),
            category: "runtime".to_string(),
            installed: node_v.is_some(),
            version: node_v.clone(),
            running: None,
            systemd_unit: None,
            actions: actions_for("node", node_v.is_some(), dispatch_script_present, false),
            notes: None,
        },
        HostServiceRow {
            id: "python3".to_string(),
            display_name: "Python 3".to_string(),
            category: "runtime".to_string(),
            installed: py_v.is_some(),
            version: py_v.clone(),
            running: None,
            systemd_unit: None,
            actions: actions_for("python3", py_v.is_some(), dispatch_script_present, false),
            notes: Some(
                "Remove uninstalls optional packages (pip/venv) only; system python3 may remain."
                    .to_string(),
            ),
        },
        HostServiceRow {
            id: "nginx".to_string(),
            display_name: "nginx".to_string(),
            category: "web".to_string(),
            installed: ngx.installed,
            version: ngx.version.clone(),
            running: nginx_running,
            systemd_unit: Some("nginx".to_string()),
            actions: actions_for("nginx", ngx.installed, dispatch_script_present, false),
            notes: Some("Full vhost editing stays on the «nginx» tab.".to_string()),
        },
        HostServiceRow {
            id: "redis".to_string(),
            display_name: "Redis".to_string(),
            category: "database".to_string(),
            installed: redis_inst,
            version: redis_v,
            running: redis_run,
            systemd_unit: Some("redis-server".to_string()),
            actions: actions_for("redis", redis_inst, dispatch_script_present, false),
            notes: None,
        },
        HostServiceRow {
            id: "postgresql".to_string(),
            display_name: "PostgreSQL".to_string(),
            category: "database".to_string(),
            installed: pg_inst,
            version: pg_v,
            running: pg_run,
            systemd_unit: Some("postgresql".to_string()),
            actions: actions_for("postgresql", pg_inst, dispatch_script_present, false),
            notes: Some(
                "Removing PostgreSQL deletes server packages and may destroy local cluster data."
                    .to_string(),
            ),
        },
        HostServiceRow {
            id: "mysql".to_string(),
            display_name: "MySQL".to_string(),
            category: "database".to_string(),
            installed: mysql_inst,
            version: mysql_v,
            running: mysql_run,
            systemd_unit: Some("mysql".to_string()),
            actions: actions_for("mysql", mysql_inst, dispatch_script_present, false),
            notes: Some("Removing MySQL may destroy databases on this host.".to_string()),
        },
        HostServiceRow {
            id: "mongodb".to_string(),
            display_name: "MongoDB".to_string(),
            category: "database".to_string(),
            installed: mongo_inst,
            version: mongo_v,
            running: mongo_run,
            systemd_unit: Some("mongod".to_string()),
            actions: actions_for("mongodb", mongo_inst, dispatch_script_present, false),
            notes: None,
        },
        HostServiceRow {
            id: "mssql".to_string(),
            display_name: "Microsoft SQL Server".to_string(),
            category: "database".to_string(),
            installed: mssql_inst,
            version: mssql_v,
            running: mssql_run,
            systemd_unit: Some("mssql-server".to_string()),
            actions: actions_for("mssql", mssql_inst, dispatch_script_present, false),
            notes: mssql_notes,
        },
        HostServiceRow {
            id: "clickhouse".to_string(),
            display_name: "ClickHouse".to_string(),
            category: "database".to_string(),
            installed: ch_inst,
            version: ch_v,
            running: ch_run,
            systemd_unit: Some("clickhouse-server".to_string()),
            actions: actions_for("clickhouse", ch_inst, dispatch_script_present, false),
            notes: None,
        },
        HostServiceRow {
            id: "oracle_client".to_string(),
            display_name: "Oracle".to_string(),
            category: "database".to_string(),
            installed: false,
            version: None,
            running: None,
            systemd_unit: None,
            actions: "none".to_string(),
            notes: oracle_notes,
        },
        HostServiceRow {
            id: "cifs_utils".to_string(),
            display_name: "CIFS utils (SMB client)".to_string(),
            category: "storage".to_string(),
            installed: cifs_inst,
            version: cifs_v,
            running: None,
            systemd_unit: None,
            actions: actions_for("cifs_utils", cifs_inst, dispatch_script_present, false),
            notes: Some("Mounting shares uses data source credentials; see SMB scripts in sudoers.".to_string()),
        },
    ];

    if !dispatch_script_present {
        for s in &mut services {
            if s.id != "oracle_client" {
                s.actions = "none".to_string();
            }
        }
    }

    HostServicesView {
        cifs_mounts: collect_cifs_mounts(),
        dispatch_script_present,
        services,
    }
}

/// Run `sudo -n pirate-host-service.sh <install|remove> <id>` (whitelist inside script).
pub fn host_service_action_via_sudo(
    action: &str,
    id: &str,
    script: &Path,
) -> Result<HostServiceActionView, ControlError> {
    let a = action.trim();
    if a != "install" && a != "remove" {
        return Err(ControlError::HostServiceOp(
            "action must be install or remove".into(),
        ));
    }
    if !host_service_id_allowed(id) {
        return Err(ControlError::HostServiceOp("unknown service id".into()));
    }
    if id == "oracle_client" {
        return Err(ControlError::HostServiceOp(
            "oracle_client cannot be installed or removed via automation".into(),
        ));
    }
    if !script.is_file() {
        return Err(ControlError::HostServiceOp(format!(
            "dispatcher not found: {}",
            script.display()
        )));
    }

    let out = Command::new("sudo")
        .args([
            "-n",
            script.to_str().ok_or_else(|| {
                ControlError::HostServiceOp("invalid dispatcher path".into())
            })?,
            a,
            id,
        ])
        .output()
        .map_err(|e| ControlError::HostServiceOp(format!("sudo: {e}")))?;

    let merged = output_text(&out);
    if !out.status.success() {
        return Ok(HostServiceActionView {
            ok: false,
            message: "host service action failed".into(),
            output: Some(merged),
        });
    }

    Ok(HostServiceActionView {
        ok: true,
        message: merged.trim().to_string(),
        output: Some(merged),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_service_id_allowed_checks() {
        assert!(host_service_id_allowed("redis"));
        assert!(!host_service_id_allowed("rm"));
    }

    #[test]
    fn first_nonempty_line_skips_blanks() {
        assert_eq!(
            first_nonempty_line("\n  \nClickHouse client version 24.1.1.2048\n"),
            Some("ClickHouse client version 24.1.1.2048".into())
        );
        assert_eq!(first_nonempty_line(""), None);
        assert_eq!(first_nonempty_line("   \n"), None);
    }

    #[test]
    fn first_nonempty_line_prefers_stderr_semantics_in_cmd_helper_doc() {
        // Simulates: stderr empty → read stdout (e.g. some tools print version only to stdout).
        let stdout = "psql (PostgreSQL) 16.2\n";
        assert_eq!(
            first_nonempty_line(stdout),
            Some("psql (PostgreSQL) 16.2".into())
        );
    }
}
