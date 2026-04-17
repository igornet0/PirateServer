//! Anti-DDoS: write JSON under `/var/lib/pirate/antiddos` and run `sudo pirate-antiddos-apply.sh`.

use crate::service::ControlError;
use crate::types::{AntiddosApplyResultView, AntiddosHostConfig, AntiddosProjectConfig, AntiddosStatsView};
use std::path::Path;
use std::process::{Command, Stdio};

fn output_text(out: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{stdout}{stderr}")
}

/// Bounds for API validation (match plan).
pub fn validate_host_config(c: &AntiddosHostConfig) -> Result<(), String> {
    if !(0.1..=1000.0).contains(&c.rate_limit_rps) {
        return Err("rate_limit_rps must be between 0.1 and 1000".into());
    }
    if !(1..=1000).contains(&c.burst) {
        return Err("burst must be between 1 and 1000".into());
    }
    if !(1..=10000).contains(&c.max_connections_per_ip) {
        return Err("max_connections_per_ip must be between 1 and 10000".into());
    }
    if !(1..=600).contains(&c.client_body_timeout_sec) {
        return Err("client_body_timeout_sec out of range".into());
    }
    if !(1..=3600).contains(&c.keepalive_timeout_sec) {
        return Err("keepalive_timeout_sec out of range".into());
    }
    if !(1..=600).contains(&c.send_timeout_sec) {
        return Err("send_timeout_sec out of range".into());
    }
    for p in &c.lockdown_app_ports.tcp_ports {
        if *p == 0 {
            return Err("invalid tcp port in lockdown".into());
        }
    }
    Ok(())
}

pub fn validate_project_config(c: &AntiddosProjectConfig) -> Result<(), String> {
    if !(0.1..=1000.0).contains(&c.rate_limit_rps) {
        return Err("rate_limit_rps must be between 0.1 and 1000".into());
    }
    if !(1..=1000).contains(&c.burst) {
        return Err("burst must be between 1 and 1000".into());
    }
    if !(1..=10000).contains(&c.max_connections_per_ip) {
        return Err("max_connections_per_ip must be between 1 and 10000".into());
    }
    Ok(())
}

pub fn default_host_config() -> AntiddosHostConfig {
    AntiddosHostConfig {
        schema_version: 1,
        engine: "nginx_nft_fail2ban".to_string(),
        enabled: false,
        aggressive: false,
        rate_limit_rps: 10.0,
        burst: 20,
        max_connections_per_ip: 30,
        client_body_timeout_sec: 12,
        keepalive_timeout_sec: 20,
        send_timeout_sec: 10,
        whitelist_cidrs: vec!["127.0.0.1/32".to_string(), "::1/128".to_string()],
        fail2ban: Default::default(),
        firewall: Default::default(),
        lockdown_app_ports: Default::default(),
    }
}

pub async fn write_host_json(path: &Path, cfg: &AntiddosHostConfig) -> Result<(), std::io::Error> {
    let raw = serde_json::to_string_pretty(cfg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    tokio::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).await?;
    tokio::fs::write(path, raw.as_bytes()).await?;
    Ok(())
}

pub async fn write_project_json(path: &Path, cfg: &AntiddosProjectConfig) -> Result<(), std::io::Error> {
    let raw = serde_json::to_string_pretty(cfg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    tokio::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).await?;
    tokio::fs::write(path, raw.as_bytes()).await?;
    Ok(())
}

pub fn apply_antiddos_via_sudo(
    script: &Path,
    state_dir: &Path,
    nginx_site_path: &Path,
) -> Result<AntiddosApplyResultView, ControlError> {
    if !script.is_file() {
        return Err(ControlError::Antiddos(format!(
            "helper not found: {}",
            script.display()
        )));
    }
    let sd = state_dir.to_str().ok_or_else(|| {
        ControlError::Antiddos("invalid antiddos state dir path".into())
    })?;
    let site = nginx_site_path.to_str().ok_or_else(|| {
        ControlError::Antiddos("invalid nginx site path".into())
    })?;
    let child = Command::new("sudo")
        .args([
            "-n",
            script.to_str().ok_or_else(|| ControlError::Antiddos("invalid script path".into()))?,
            sd,
            site,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ControlError::Antiddos(format!("sudo: {e}")))?;
    let out = child.wait_with_output().map_err(|e| ControlError::Antiddos(format!("sudo: {e}")))?;
    let txt = output_text(&out);
    Ok(AntiddosApplyResultView {
        ok: out.status.success(),
        message: if out.status.success() {
            "apply finished".into()
        } else {
            "apply failed".into()
        },
        stderr: if txt.trim().is_empty() {
            None
        } else {
            Some(txt)
        },
    })
}

pub fn read_host_json(path: &Path) -> Result<AntiddosHostConfig, ControlError> {
    if !path.is_file() {
        return Ok(default_host_config());
    }
    let raw = std::fs::read_to_string(path).map_err(ControlError::Io)?;
    if raw.trim().is_empty() {
        return Ok(default_host_config());
    }
    serde_json::from_str(&raw).map_err(|e| ControlError::Antiddos(format!("host json: {e}")))
}

/// Best-effort stats for `GET /api/v1/antiddos/stats` (same host as control-api).
pub fn collect_antiddos_stats(limit_log: &Path) -> AntiddosStatsView {
    let mut fail2ban_jail: Option<String> = None;
    let mut fail2ban_banned: Option<u32> = None;
    if let Ok(out) = Command::new("fail2ban-client")
        .args(["status", "pirate-nginx-dos"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            fail2ban_jail = Some("pirate-nginx-dos".into());
            for line in s.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("Currently banned:") {
                    if let Ok(n) = rest.trim().parse::<u32>() {
                        fail2ban_banned = Some(n);
                    }
                }
            }
        }
    }

    let mut limit_log_tail: Vec<String> = Vec::new();
    if let Ok(raw) = std::fs::read_to_string(limit_log) {
        let lines: Vec<&str> = raw.lines().collect();
        let n = lines.len();
        let start = n.saturating_sub(50);
        for l in &lines[start..] {
            limit_log_tail.push((*l).to_string());
        }
    }

    let nft_ok = Command::new("nft")
        .args(["list", "table", "inet", "pirate_antiddos"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    AntiddosStatsView {
        fail2ban_jail,
        fail2ban_banned,
        limit_log_tail,
        nft_table_present: nft_ok,
    }
}
