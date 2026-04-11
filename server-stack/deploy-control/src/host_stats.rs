//! Host metrics for the control-api machine (disk for `deploy_root`, RAM, CPU, load, sensors).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{
    Components, CpuRefreshKind, Disks, MemoryRefreshKind, NetworkData, Networks,
    ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL,
};

use crate::types::{HostLogLine, HostMountStats, HostNetInterface, HostStatsView};

/// Previous raw network counters for rate computation (stored in control-api between requests).
#[derive(Debug, Clone)]
pub struct NetCounters {
    pub at: Instant,
    pub rx: HashMap<String, u64>,
    pub tx: HashMap<String, u64>,
}

fn canonical_root(deploy_root: &Path) -> PathBuf {
    std::fs::canonicalize(deploy_root).unwrap_or_else(|_| deploy_root.to_path_buf())
}

fn disk_for_path(deploy_root: &Path) -> (u64, u64, String) {
    let deploy_root = canonical_root(deploy_root);
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64, u64, String)> = None;
    for disk in disks.list() {
        let mp = disk.mount_point();
        if deploy_root.starts_with(mp) {
            let key_len = mp.as_os_str().len();
            let total = disk.total_space();
            let avail = disk.available_space();
            let path_str = mp.display().to_string();
            let replace = best
                .as_ref()
                .map(|(best_len, _, _, _)| key_len > *best_len)
                .unwrap_or(true);
            if replace {
                best = Some((key_len, total, avail, path_str));
            }
        }
    }
    if let Some((_, total, avail, path)) = best {
        return (total, avail, path);
    }
    // Fallback: root mount or first disk.
    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        if disk.mount_point() == Path::new("/") {
            return (
                disk.total_space(),
                disk.available_space(),
                "/".to_string(),
            );
        }
    }
    if let Some(disk) = disks.list().first() {
        return (
            disk.total_space(),
            disk.available_space(),
            disk.mount_point().display().to_string(),
        );
    }
    (0, 0, "?".to_string())
}

pub(crate) fn all_mounts() -> Vec<HostMountStats> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|d| HostMountStats {
            path: d.mount_point().display().to_string(),
            total_bytes: d.total_space(),
            free_bytes: d.available_space(),
        })
        .collect()
}

fn tail_app_log(path: &Path, max_lines: usize) -> Vec<HostLogLine> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if meta.len() == 0 {
        return Vec::new();
    }
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let lossy = String::from_utf8_lossy(&data);
    let mut lines: Vec<&str> = lossy.lines().collect();
    if lines.len() > max_lines {
        lines = lines[lines.len().saturating_sub(max_lines)..].to_vec();
    }
    lines
        .into_iter()
        .filter_map(|line| {
            let level = if line.contains(" ERROR ") || line.contains("error") {
                "error"
            } else if line.contains(" WARN ") || line.contains("warn") {
                "warn"
            } else {
                "info"
            };
            Some(HostLogLine {
                ts_ms: chrono::Utc::now().timestamp_millis(),
                level: level.to_string(),
                message: line.chars().take(500).collect(),
            })
        })
        .collect()
}

/// Collects CPU, memory, process stats, all mounts, optional log tail, and network rates.
/// Network throughput uses two samples in one call (~100ms apart), so the first poll is not all zeros.
/// `net_prev` is kept for API compatibility; rates no longer depend on it.
pub fn collect_host_stats(
    deploy_root: &Path,
    _net_prev: Option<&NetCounters>,
    log_path: Option<&Path>,
) -> (HostStatsView, NetCounters) {
    let disk_mounts = all_mounts();
    let (disk_total_bytes, disk_free_bytes, disk_mount_path) = disk_for_path(deploy_root);

    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_memory(MemoryRefreshKind::everything())
            .with_cpu(CpuRefreshKind::everything())
            .with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_memory();
    let memory_total_bytes = sys.total_memory();
    let memory_used_bytes = sys.used_memory();

    sys.refresh_cpu_usage();
    thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_usage();
    let cpu_usage_percent = sys.global_cpu_usage();

    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );
    let process_count = sys.processes().len();

    let load = System::load_average();

    let components = Components::new_with_refreshed_list();
    let mut temps: Vec<f32> = Vec::new();
    for c in components.iter() {
        let t = c.temperature();
        if t.is_finite() && t > 0.0 && t < 250.0 {
            temps.push(t);
        }
    }
    let (temperature_current_celsius, temperature_avg_celsius) = if temps.is_empty() {
        (None, None)
    } else {
        let current = temps.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let avg = temps.iter().sum::<f32>() / temps.len() as f32;
        (Some(current), Some(avg))
    };

    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh();
    let net_t0 = Instant::now();
    let mut rx_first = HashMap::new();
    let mut tx_first = HashMap::new();
    for (name, data) in networks.iter() {
        let d: &NetworkData = data;
        rx_first.insert(name.clone(), d.received());
        tx_first.insert(name.clone(), d.transmitted());
    }
    thread::sleep(Duration::from_millis(100));
    networks.refresh();
    let now = Instant::now();
    let dt = now.duration_since(net_t0).as_secs_f64().max(0.001);

    let mut rx_map = HashMap::new();
    let mut tx_map = HashMap::new();
    let mut network_interfaces = Vec::new();
    for (name, data) in networks.iter() {
        let name = name.clone();
        let d: &NetworkData = data;
        let r = d.received();
        let t = d.transmitted();
        rx_map.insert(name.clone(), r);
        tx_map.insert(name.clone(), t);

        let pr = rx_first.get(&name).copied().unwrap_or(r);
        let pt = tx_first.get(&name).copied().unwrap_or(t);
        let rx_bps = (r.saturating_sub(pr)) as f64 / dt;
        let tx_bps = (t.saturating_sub(pt)) as f64 / dt;

        network_interfaces.push(HostNetInterface {
            name,
            rx_bytes_per_s: rx_bps,
            tx_bytes_per_s: tx_bps,
            rx_errors: d.errors_on_received(),
            tx_errors: d.errors_on_transmitted(),
        });
    }

    let net_counters = NetCounters {
        at: now,
        rx: rx_map,
        tx: tx_map,
    };

    let log_tail = log_path
        .map(|p| tail_app_log(p, 40))
        .unwrap_or_default();

    let view = HostStatsView {
        disk_free_bytes,
        disk_total_bytes,
        disk_mount_path,
        memory_used_bytes,
        memory_total_bytes,
        cpu_usage_percent,
        load_average_1m: load.one,
        load_average_5m: load.five,
        load_average_15m: load.fifteen,
        temperature_current_celsius,
        temperature_avg_celsius,
        process_count,
        disk_mounts,
        network_interfaces,
        log_tail,
    };

    (view, net_counters)
}
