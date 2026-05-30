//! Host software inventory (Node, Python, nginx, DB packages) for dashboard «Services» tab.

use crate::nginx::collect_nginx_status;
use crate::types::{
    HostServiceActionView, HostServiceRow, HostServiceRuntimeConfigView, HostServicesView,
    NginxStatusView,
};
use crate::ControlError;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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
    "minio",
    "meilisearch",
    "stack_tun_api",
];

pub fn host_service_id_allowed(id: &str) -> bool {
    HOST_SERVICE_IDS.iter().any(|s| *s == id)
}

fn output_text(out: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{stdout}{stderr}")
}

/// Suffix for operator-visible errors when non-interactive sudo fails for host-service.
const HOST_SERVICE_SUDOERS_HINT: &str = "\n\nHint: control-api runs host-service actions as user `pirate` with `sudo -n`. Install with env (MinIO, Meilisearch, PostgreSQL explorer password, …) requires in /etc/sudoers.d/99-pirate-smb a line:\n  pirate ALL=(root) NOPASSWD:SETENV: /usr/local/lib/pirate/pirate-host-service.sh\nRe-run install.sh from an updated bundle or apply OTA so this fragment is installed; then `sudo -u pirate sudo -n PIRATE_SUDO_NOPASSWD_CHECK=1 /usr/local/lib/pirate/pirate-host-service.sh show-runtime minio` should succeed without a password prompt.";

fn augment_host_service_sudo_error(merged: &str) -> String {
    let m = merged.to_lowercase();
    if m.contains("password is required")
        || m.contains("a terminal is required to read the password")
        || m.contains("no tty present")
    {
        format!("{merged}{HOST_SERVICE_SUDOERS_HINT}")
    } else {
        merged.to_string()
    }
}

/// Verifies non-interactive sudo allows command-line env for the dispatcher (`NOPASSWD:SETENV` in sudoers).
fn preflight_host_service_install_env_sudo(script: &Path) -> Result<(), ControlError> {
    let script_s = script
        .to_str()
        .ok_or_else(|| ControlError::HostServiceOp("invalid dispatcher path".into()))?;
    let out = Command::new("sudo")
        .args([
            "-n",
            "PIRATE_SUDO_NOPASSWD_CHECK=1",
            script_s,
            "show-runtime",
            "minio",
        ])
        .output()
        .map_err(|e| ControlError::HostServiceOp(format!("sudo preflight: {e}")))?;
    if !out.status.success() {
        let merged = output_text(&out);
        return Err(ControlError::HostServiceOp(format!(
            "sudo preflight failed (host-service install with env needs NOPASSWD:SETENV for {script_s} in /etc/sudoers.d/99-pirate-smb): {}",
            augment_host_service_sudo_error(&merged)
        )));
    }
    Ok(())
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

/// Whether the unit is enabled to start at boot (`systemctl is-enabled --quiet`).
fn systemctl_is_enabled(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["is-enabled", "--quiet", unit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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
    nginx_ops: &Path,
    dispatch_script: &Path,
) -> HostServicesView {
    let dispatch_script_present = dispatch_script.is_file();
    let ngx: NginxStatusView =
        collect_nginx_status(nginx_site_path, nginx_ensure, nginx_apply, nginx_ops);
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

    // `pirate-minio.service` from install-minio.sh (binary may live outside PATH).
    let minio_inst = Path::new("/usr/local/bin/minio").is_file() || has_command("minio");
    let minio_v = if Path::new("/usr/local/bin/minio").is_file() {
        cmd_stderr_first_line("/usr/local/bin/minio", &["--version"])
            .or_else(|| cmd_stdout_trim("/usr/local/bin/minio", &["--version"]))
    } else {
        cmd_stderr_first_line("minio", &["--version"])
            .or_else(|| cmd_stdout_trim("minio", &["--version"]))
    };
    let minio_run = systemctl_is_active("pirate-minio");

    let meili_inst = Path::new("/usr/local/bin/meilisearch").is_file() || has_command("meilisearch");
    let meili_v = if Path::new("/usr/local/bin/meilisearch").is_file() {
        cmd_stderr_first_line("/usr/local/bin/meilisearch", &["--version"])
            .or_else(|| cmd_stdout_trim("/usr/local/bin/meilisearch", &["--version"]))
    } else {
        cmd_stderr_first_line("meilisearch", &["--version"])
            .or_else(|| cmd_stdout_trim("meilisearch", &["--version"]))
    };
    let meili_run = systemctl_is_active("pirate-meilisearch");

    let st_bin = Path::new("/usr/local/bin/stack-tun-api");
    let st_unit = Path::new("/etc/systemd/system/pirate-stack-tun-api.service");
    let st_deployed = st_bin.is_file() && st_unit.is_file();
    let st_enabled = st_deployed && systemctl_is_enabled("pirate-stack-tun-api");
    let st_v = if st_bin.is_file() {
        cmd_stdout_trim("/usr/local/bin/stack-tun-api", &["--version"]).or_else(|| Some("present".into()))
    } else {
        None
    };
    let st_run = systemctl_is_active("pirate-stack-tun-api");

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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
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
            runtime_configurable: false,
        },
        HostServiceRow {
            id: "minio".to_string(),
            display_name: "MinIO (S3)".to_string(),
            category: "storage".to_string(),
            installed: minio_inst,
            version: minio_v,
            running: minio_run,
            systemd_unit: Some("pirate-minio".to_string()),
            actions: actions_for("minio", minio_inst, dispatch_script_present, false),
            notes: Some(
                "API 127.0.0.1:9000, console 127.0.0.1:9001. Credentials: /etc/pirate-minio.env."
                    .to_string(),
            ),
            runtime_configurable: true,
        },
        HostServiceRow {
            id: "meilisearch".to_string(),
            display_name: "Meilisearch".to_string(),
            category: "search".to_string(),
            installed: meili_inst,
            version: meili_v,
            running: meili_run,
            systemd_unit: Some("pirate-meilisearch".to_string()),
            actions: actions_for("meilisearch", meili_inst, dispatch_script_present, false),
            notes: Some("HTTP 127.0.0.1:7700. Master key: /etc/pirate-meilisearch.env.".to_string()),
            runtime_configurable: true,
        },
        HostServiceRow {
            id: "stack_tun_api".to_string(),
            display_name: "Stack tunnel API".to_string(),
            category: "tunnel".to_string(),
            // `installed` here means the unit is **enabled** (UI: Install → enable & start, Remove → disable & stop).
            installed: st_enabled,
            version: st_v,
            running: st_run,
            systemd_unit: Some("pirate-stack-tun-api".to_string()),
            actions: if st_deployed {
                actions_for("stack_tun_api", st_enabled, dispatch_script_present, false)
            } else {
                "none".to_string()
            },
            notes: Some(if st_deployed {
                "HTTP 0.0.0.0:9380, gRPC 0.0.0.0:9381 (LAN; /etc/pirate-stack-tun-api.env). Disabled by default after stack install; use Install to enable."
                    .to_string()
            } else {
                let mut parts = vec![
                    "No Enable button until both files exist: /usr/local/bin/stack-tun-api and /etc/systemd/system/pirate-stack-tun-api.service."
                        .to_string(),
                ];
                if !st_bin.is_file() {
                    parts.push("Missing: stack-tun-api binary.".to_string());
                }
                if !st_unit.is_file() {
                    parts.push("Missing: pirate-stack-tun-api.service unit.".to_string());
                }
                parts.push(
                    "Apply install.sh from a recent Linux bundle or upload a server-stack OTA tarball that installs these; then refresh this list."
                        .to_string(),
                );
                parts.join(" ")
            }),
            runtime_configurable: false,
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

const MAX_HOST_SERVICE_ENV_VALUE_BYTES: usize = 8192;

fn sanitize_host_service_install_value(v: &str) -> Result<String, ControlError> {
    if v.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
        return Err(ControlError::HostServiceOp(
            "install env value must be a single line (no newlines or NUL)".into(),
        ));
    }
    if v.len() > MAX_HOST_SERVICE_ENV_VALUE_BYTES {
        return Err(ControlError::HostServiceOp("install env value too long".into()));
    }
    Ok(v.to_string())
}

fn allowed_install_env_key(id: &str, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return false;
    }
    match id {
        "minio" => matches!(
            key,
            "MINIO_ROOT_USER"
                | "MINIO_ROOT_PASSWORD"
                | "PIRATE_MINIO_API_ADDR"
                | "PIRATE_MINIO_CONSOLE_ADDR"
                | "PIRATE_MINIO_DATA_DIR"
        ),
        "meilisearch" => matches!(
            key,
            "MEILI_MASTER_KEY"
                | "PIRATE_MEILI_HTTP_ADDR"
                | "PIRATE_MEILI_DB_PATH"
                | "PIRATE_MEILISEARCH_VERSION"
        ),
        "redis" => matches!(
            key,
            "PIRATE_REDIS_BIND"
                | "PIRATE_REDIS_PORT"
                | "PIRATE_REDIS_AUTH_MODE"
                | "PIRATE_REDIS_PASSWORD"
                | "PIRATE_REDIS_ACL_USERNAME"
        ),
        "postgresql" => matches!(
            key,
            "PIRATE_POSTGRESQL_LISTEN_ADDRESSES"
                | "PIRATE_POSTGRESQL_PORT"
                | "PIRATE_EXPLORER_DB_USER"
                | "PIRATE_EXPLORER_DB_NAME"
                | "PIRATE_EXPLORER_DB_PASSWORD"
                | "PIRATE_EXPLORER_DB_HOST"
                | "PIRATE_EXPLORER_DB_PORT"
        ),
        _ => false,
    }
}

/// Filters and sanitizes install-time env for `sudo -n KEY=val … pirate-host-service.sh install <id>`.
/// Unknown keys for this service are dropped. Empty values are dropped.
pub fn filter_host_service_install_env(
    id: &str,
    input: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ControlError> {
    let mut out = BTreeMap::new();
    for (k, v) in input {
        let k = k.trim();
        if v.is_empty() {
            continue;
        }
        if !allowed_install_env_key(id, k) {
            continue;
        }
        let v = sanitize_host_service_install_value(v.trim())?;
        out.insert(k.to_string(), v);
    }
    validate_host_service_install_constraints(id, &out)?;
    Ok(out)
}

fn validate_host_service_install_constraints(
    id: &str,
    m: &BTreeMap<String, String>,
) -> Result<(), ControlError> {
    match id {
        "redis" => {
            if let Some(mode) = m.get("PIRATE_REDIS_AUTH_MODE") {
                if mode != "requirepass" && mode != "acl" {
                    return Err(ControlError::HostServiceOp(
                        "PIRATE_REDIS_AUTH_MODE must be requirepass or acl".into(),
                    ));
                }
                if mode == "acl"
                    && m.get("PIRATE_REDIS_ACL_USERNAME")
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true)
                {
                    return Err(ControlError::HostServiceOp(
                        "PIRATE_REDIS_ACL_USERNAME is required when PIRATE_REDIS_AUTH_MODE=acl".into(),
                    ));
                }
            }
            if let Some(p) = m.get("PIRATE_REDIS_PORT") {
                let n: u16 = p.parse().map_err(|_| {
                    ControlError::HostServiceOp("PIRATE_REDIS_PORT must be a number 1–65535".into())
                })?;
                if n == 0 {
                    return Err(ControlError::HostServiceOp(
                        "PIRATE_REDIS_PORT must be a number 1–65535".into(),
                    ));
                }
            }
        }
        "postgresql" => {
            if let Some(p) = m.get("PIRATE_POSTGRESQL_PORT") {
                let n: u16 = p.parse().map_err(|_| {
                    ControlError::HostServiceOp(
                        "PIRATE_POSTGRESQL_PORT must be a number 1–65535".into(),
                    )
                })?;
                if n == 0 {
                    return Err(ControlError::HostServiceOp(
                        "PIRATE_POSTGRESQL_PORT must be a number 1–65535".into(),
                    ));
                }
            }
            if let Some(p) = m.get("PIRATE_EXPLORER_DB_PORT") {
                let n: u16 = p.parse().map_err(|_| {
                    ControlError::HostServiceOp(
                        "PIRATE_EXPLORER_DB_PORT must be a number 1–65535".into(),
                    )
                })?;
                if n == 0 {
                    return Err(ControlError::HostServiceOp(
                        "PIRATE_EXPLORER_DB_PORT must be a number 1–65535".into(),
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Run `sudo -n [KEY=val …] pirate-host-service.sh <install|remove> <id>` (whitelist inside script).
/// Install env is passed as sudo `VAR=value` assignments before the script path (requires `NOPASSWD:SETENV` in sudoers).
pub fn host_service_action_via_sudo(
    action: &str,
    id: &str,
    script: &Path,
    install_env: Option<BTreeMap<String, String>>,
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
    if a == "remove" && install_env.is_some() {
        return Err(ControlError::HostServiceOp(
            "remove does not accept install env".into(),
        ));
    }

    let script_s = script
        .to_str()
        .ok_or_else(|| ControlError::HostServiceOp("invalid dispatcher path".into()))?;

    let mut cmd = Command::new("sudo");
    cmd.arg("-n");
    if a == "install" {
        if let Some(m) = install_env {
            let filtered = filter_host_service_install_env(id, m)?;
            if !filtered.is_empty() {
                preflight_host_service_install_env_sudo(script)?;
                for (k, v) in &filtered {
                    cmd.arg(format!("{k}={v}"));
                }
            }
        }
    }
    cmd.args([script_s, a, id]);

    let out = cmd
        .output()
        .map_err(|e| ControlError::HostServiceOp(format!("sudo: {e}")))?;

    let merged = output_text(&out);
    if !out.status.success() {
        let merged = augment_host_service_sudo_error(&merged);
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

/// Parse `KEY=VAL` / `export KEY=VAL` lines (basic `.env` subset used by pirate hosts).
pub fn parse_host_service_env_file(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, rest)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        let rest = rest.trim();
        let v = if (rest.starts_with('"') && rest.ends_with('"')) || (rest.starts_with('\'') && rest.ends_with('\''))
        {
            let inner = &rest[1..rest.len() - 1];
            inner.replace("\\\"", "\"").replace("\\\\", "\\")
        } else {
            rest.to_string()
        };
        m.insert(k.to_string(), v);
    }
    m
}

/// Write env map to a file body suitable for `/etc/pirate-*.env`.
pub fn format_host_service_env_file(m: &BTreeMap<String, String>) -> String {
    let mut s = String::new();
    for (k, v) in m {
        let needs_quote = v.is_empty()
            || v.chars()
                .any(|c| c.is_whitespace() || c == '"' || c == '\'' || c == '\\' || c == '=');
        if needs_quote {
            let esc = v.replace('\\', "\\\\").replace('"', "\\\"");
            s.push_str(&format!("{}=\"{}\"\n", k, esc));
        } else {
            s.push_str(&format!("{}={}\n", k, v));
        }
    }
    s
}

fn runtime_config_id_supported(id: &str) -> bool {
    matches!(id, "minio" | "meilisearch")
}

/// `sudo pirate-host-service.sh show-runtime <id>` → raw env file text.
pub fn host_service_show_runtime_via_sudo(id: &str, script: &Path) -> Result<String, ControlError> {
    if !runtime_config_id_supported(id) {
        return Err(ControlError::HostServiceOp(
            "runtime config is only available for minio and meilisearch".into(),
        ));
    }
    if !host_service_id_allowed(id) {
        return Err(ControlError::HostServiceOp("unknown service id".into()));
    }
    if !script.is_file() {
        return Err(ControlError::HostServiceOp(format!(
            "dispatcher not found: {}",
            script.display()
        )));
    }
    let script_s = script
        .to_str()
        .ok_or_else(|| ControlError::HostServiceOp("invalid dispatcher path".into()))?;
    let out = Command::new("sudo")
        .args(["-n", script_s, "show-runtime", id])
        .output()
        .map_err(|e| ControlError::HostServiceOp(format!("sudo: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let merged = augment_host_service_sudo_error(&output_text(&out));
        return Err(ControlError::HostServiceOp(format!(
            "show-runtime failed: {merged}"
        )));
    }
    Ok(text)
}

/// Rebuild env file and restart (apply-runtime).
pub fn host_service_apply_runtime_via_sudo(
    id: &str,
    script: &Path,
    input: BTreeMap<String, String>,
) -> Result<HostServiceActionView, ControlError> {
    if !runtime_config_id_supported(id) {
        return Err(ControlError::HostServiceOp(
            "runtime apply is only for minio and meilisearch".into(),
        ));
    }
    if !host_service_id_allowed(id) {
        return Err(ControlError::HostServiceOp("unknown service id".into()));
    }
    if !script.is_file() {
        return Err(ControlError::HostServiceOp(format!(
            "dispatcher not found: {}",
            script.display()
        )));
    }
    let filtered = filter_host_service_install_env(id, input)?;
    if filtered.is_empty() {
        return Err(ControlError::HostServiceOp(
            "runtime env is empty after filtering".into(),
        ));
    }
    let body = format_host_service_env_file(&filtered);
    let script_s = script
        .to_str()
        .ok_or_else(|| ControlError::HostServiceOp("invalid dispatcher path".into()))?;

    let mut child = Command::new("sudo")
        .args(["-n", script_s, "apply-runtime", id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ControlError::HostServiceOp(format!("sudo: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .map_err(|e| ControlError::HostServiceOp(format!("write stdin: {e}")))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| ControlError::HostServiceOp(format!("wait: {e}")))?;
    let merged = output_text(&out);
    if !out.status.success() {
        let merged = augment_host_service_sudo_error(&merged);
        return Ok(HostServiceActionView {
            ok: false,
            message: "apply-runtime failed".into(),
            output: Some(merged),
        });
    }
    Ok(HostServiceActionView {
        ok: true,
        message: merged.trim().to_string(),
        output: Some(merged),
    })
}

/// `systemctl restart` only (no file write).
pub fn host_service_restart_runtime_via_sudo(id: &str, script: &Path) -> Result<HostServiceActionView, ControlError> {
    if !runtime_config_id_supported(id) {
        return Err(ControlError::HostServiceOp(
            "restart is only supported for minio and meilisearch".into(),
        ));
    }
    if !host_service_id_allowed(id) {
        return Err(ControlError::HostServiceOp("unknown service id".into()));
    }
    if !script.is_file() {
        return Err(ControlError::HostServiceOp(format!(
            "dispatcher not found: {}",
            script.display()
        )));
    }
    let script_s = script
        .to_str()
        .ok_or_else(|| ControlError::HostServiceOp("invalid dispatcher path".into()))?;
    let out = Command::new("sudo")
        .args(["-n", script_s, "restart", id])
        .output()
        .map_err(|e| ControlError::HostServiceOp(format!("sudo: {e}")))?;
    let merged = output_text(&out);
    if !out.status.success() {
        let merged = augment_host_service_sudo_error(&merged);
        return Ok(HostServiceActionView {
            ok: false,
            message: "restart failed".into(),
            output: Some(merged),
        });
    }
    Ok(HostServiceActionView {
        ok: true,
        message: merged.trim().to_string(),
        output: Some(merged),
    })
}

/// `GET` body for `HostServiceRuntimeConfigView`.
pub fn collect_host_service_runtime_config(
    id: &str,
    script: &Path,
) -> Result<HostServiceRuntimeConfigView, ControlError> {
    let text = host_service_show_runtime_via_sudo(id, script)?;
    let env = parse_host_service_env_file(&text);
    Ok(HostServiceRuntimeConfigView { env })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn host_service_id_allowed_checks() {
        assert!(host_service_id_allowed("redis"));
        assert!(host_service_id_allowed("minio"));
        assert!(host_service_id_allowed("meilisearch"));
        assert!(host_service_id_allowed("stack_tun_api"));
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

    #[test]
    fn filter_install_env_drops_bad_keys_and_allows_minio() {
        let mut m = BTreeMap::new();
        m.insert("MINIO_ROOT_USER".into(), "a".into());
        m.insert("PIRATE_MINIO_API_ADDR".into(), "127.0.0.1:9000".into());
        m.insert("EVIL".into(), "nope".into());
        let out = filter_host_service_install_env("minio", m).expect("ok");
        assert_eq!(out.get("MINIO_ROOT_USER"), Some(&"a".to_string()));
        assert_eq!(out.get("PIRATE_MINIO_API_ADDR"), Some(&"127.0.0.1:9000".to_string()));
        assert!(!out.contains_key("EVIL"));
    }

    #[test]
    fn filter_install_env_rejects_newline_in_value() {
        let mut m = BTreeMap::new();
        m.insert("MINIO_ROOT_PASSWORD".into(), "x\ny".into());
        let err = filter_host_service_install_env("minio", m).err();
        assert!(err.is_some());
    }

    #[test]
    fn filter_install_env_redis_acl_requires_username() {
        let mut m = BTreeMap::new();
        m.insert("PIRATE_REDIS_BIND".into(), "127.0.0.1".into());
        m.insert("PIRATE_REDIS_PORT".into(), "6379".into());
        m.insert("PIRATE_REDIS_AUTH_MODE".into(), "acl".into());
        assert!(filter_host_service_install_env("redis", m).is_err());
    }

    #[test]
    fn filter_install_env_postgresql_ports() {
        let mut m = BTreeMap::new();
        m.insert("PIRATE_POSTGRESQL_PORT".into(), "5432".into());
        m.insert("PIRATE_EXPLORER_DB_HOST".into(), "127.0.0.1".into());
        m.insert("PIRATE_EXPLORER_DB_PORT".into(), "5432".into());
        let out = filter_host_service_install_env("postgresql", m).expect("ok");
        assert_eq!(out.get("PIRATE_POSTGRESQL_PORT"), Some(&"5432".to_string()));
    }

    #[test]
    fn env_file_format_roundtrip() {
        let mut m = BTreeMap::new();
        m.insert("A".into(), "b=c".into());
        m.insert("X".into(), "y".into());
        let s = format_host_service_env_file(&m);
        let p = parse_host_service_env_file(&s);
        assert_eq!(p.get("A"), Some(&"b=c".to_string()));
        assert_eq!(p.get("X"), Some(&"y".to_string()));
    }
}
