//! On-demand heavy host metrics (CPU tops, memory, disk IO by process, network, process list).

use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, NetworkData, Networks, ProcessRefreshKind,
    ProcessesToUpdate, RefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL,
};

use crate::host_stats::{all_mounts, NetCounters};
use crate::types::{
    CpuDetail, CpuTimes, DiskDetail, DiskIoSummary, HostNetInterface, LoadAvg, MemoryDetail,
    MemoryOverview, NetworkDetail, ProcessCpu, ProcessDisk, ProcessMem, ProcessRow, ProcessesDetail,
    SeriesHint,
};

#[cfg(target_os = "linux")]
fn linux_mem_extra() -> (Option<u64>, Option<u64>) {
    let Ok(s) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let mut cached = None;
    let mut buffers = None;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("Cached:") {
            if let Some(kb) = rest.trim().split_whitespace().next() {
                cached = kb.parse::<u64>().ok().map(|k| k * 1024);
            }
        }
        if let Some(rest) = line.strip_prefix("Buffers:") {
            if let Some(kb) = rest.trim().split_whitespace().next() {
                buffers = kb.parse::<u64>().ok().map(|k| k * 1024);
            }
        }
    }
    (cached, buffers)
}

#[cfg(not(target_os = "linux"))]
fn linux_mem_extra() -> (Option<u64>, Option<u64>) {
    (None, None)
}

/// First line of /proc/stat for aggregate jiffies (Linux only).
#[cfg(target_os = "linux")]
fn linux_cpu_times_jiff() -> Option<(u64, u64, u64)> {
    let s = std::fs::read_to_string("/proc/stat").ok()?;
    let line = s.lines().next()?;
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    let user: u64 = it.next()?.parse().ok()?;
    let nice: u64 = it.next()?.parse().ok()?;
    let system: u64 = it.next()?.parse().ok()?;
    let idle: u64 = it.next()?.parse().ok()?;
    let user_total = user.saturating_add(nice);
    Some((
        user_total.saturating_mul(10),
        system.saturating_mul(10),
        idle.saturating_mul(10),
    ))
}

#[cfg(not(target_os = "linux"))]
fn linux_cpu_times_jiff() -> Option<(u64, u64, u64)> {
    None
}

pub fn collect_cpu_detail(top_n: usize) -> CpuDetail {
    let load = System::load_average();
    let loadavg = LoadAvg {
        m1: load.one,
        m5: load.five,
        m15: load.fifteen,
    };

    let times = linux_cpu_times_jiff().map(|(u, s, i)| CpuTimes {
        user_ms: u,
        system_ms: s,
        idle_ms: i,
    });

    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_cpu_usage();
    thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_usage();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    let mut procs: Vec<(sysinfo::Pid, f32, String)> = sys
        .processes()
        .iter()
        .map(|(pid, p)| (*pid, p.cpu_usage(), p.name().to_string_lossy().to_string()))
        .collect();
    procs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top_processes: Vec<ProcessCpu> = procs
        .into_iter()
        .take(top_n)
        .map(|(pid, cpu, name)| ProcessCpu {
            pid: pid.as_u32(),
            name,
            cpu_percent: cpu,
        })
        .collect();

    CpuDetail {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        loadavg,
        times,
        top_processes,
        series_hint: SeriesHint {
            available_ranges: vec![
                "15m".to_string(),
                "1h".to_string(),
                "24h".to_string(),
            ],
        },
    }
}

pub fn collect_memory_detail(top_n: usize) -> MemoryDetail {
    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_memory(MemoryRefreshKind::everything())
            .with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_memory();
    let (cached, buffers) = linux_mem_extra();
    let memory = MemoryOverview {
        total_bytes: sys.total_memory(),
        used_bytes: sys.used_memory(),
        available_bytes: sys.available_memory(),
        cached_bytes: cached,
        buffers_bytes: buffers,
        swap_total_bytes: sys.total_swap(),
        swap_used_bytes: sys.used_swap(),
    };

    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );
    let mut procs: Vec<(sysinfo::Pid, u64, String)> = sys
        .processes()
        .iter()
        .map(|(pid, p)| (*pid, p.memory(), p.name().to_string_lossy().to_string()))
        .collect();
    procs.sort_by(|a, b| b.1.cmp(&a.1));
    let top_processes: Vec<ProcessMem> = procs
        .into_iter()
        .take(top_n)
        .map(|(pid, mem, name)| ProcessMem {
            pid: pid.as_u32(),
            name,
            memory_bytes: mem,
        })
        .collect();

    MemoryDetail {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        memory,
        top_processes,
    }
}

pub fn collect_disk_detail(top_n: usize) -> DiskDetail {
    let mounts = all_mounts();
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );
    let mut procs: Vec<(sysinfo::Pid, u64, u64, String)> = sys
        .processes()
        .iter()
        .map(|(pid, p)| {
            let du = p.disk_usage();
            (
                *pid,
                du.read_bytes,
                du.written_bytes,
                p.name().to_string_lossy().to_string(),
            )
        })
        .collect();
    procs.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)));
    let top_processes: Vec<ProcessDisk> = procs
        .into_iter()
        .take(top_n)
        .map(|(pid, r, w, name)| ProcessDisk {
            pid: pid.as_u32(),
            name,
            read_bytes: r,
            write_bytes: w,
        })
        .collect();

    DiskDetail {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        mounts,
        io: Some(DiskIoSummary {
            note: "Cross-platform disk IO rates are not aggregated here; use per-process counters as a hint.",
        }),
        top_processes,
    }
}

pub fn collect_network_detail(_prev: Option<&NetCounters>) -> (NetworkDetail, NetCounters) {
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
    let mut interfaces = Vec::new();
    for (name, data) in networks.iter() {
        let d: &NetworkData = data;
        let r = d.received();
        let t = d.transmitted();
        rx_map.insert(name.clone(), r);
        tx_map.insert(name.clone(), t);

        let pr = rx_first.get(name).copied().unwrap_or(r);
        let pt = tx_first.get(name).copied().unwrap_or(t);
        let rx_bps = (r.saturating_sub(pr)) as f64 / dt;
        let tx_bps = (t.saturating_sub(pt)) as f64 / dt;
        interfaces.push(HostNetInterface {
            name: name.clone(),
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

    let detail = NetworkDetail {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        interfaces,
        connections_note: "Active connection listing is not implemented (expensive); use OS tools if needed.",
    };
    (detail, net_counters)
}

pub fn collect_processes_list(query: &str, limit: usize) -> ProcessesDetail {
    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_cpu_usage();
    thread::sleep(Duration::from_millis(50));
    sys.refresh_cpu_usage();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    let q = query.trim().to_lowercase();
    let mut rows: Vec<ProcessRow> = sys
        .processes()
        .iter()
        .filter(|(_, p)| {
            if q.is_empty() {
                return true;
            }
            p.name().to_string_lossy().to_lowercase().contains(&q)
        })
        .map(|(pid, p)| ProcessRow {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().to_string(),
            cpu_percent: p.cpu_usage(),
            memory_bytes: p.memory(),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = rows.len();
    rows.truncate(limit);

    ProcessesDetail {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        processes: rows,
        total,
    }
}
