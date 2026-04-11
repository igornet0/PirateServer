//! Remote host metrics via `GetHostStats` / `GetHostStatsDetail` (deploy-server gRPC).

use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::{
    host_stats_detail_response::Detail as DetailOneof, CpuDetailProto, DiskDetailProto,
    HostStatsDetailKind, HostStatsDetailRequest, HostStatsDetailResponse, HostStatsRequest,
    HostStatsResponse, MemoryDetailProto, NetworkDetailProto, ProcessesDetailProto,
};
use deploy_proto::DeployServiceClient;
use serde_json::{json, Value};
use tonic::Request;

use crate::connection::{load_endpoint, load_project_id, load_signing_key_for_endpoint};

fn attach_if_paired<T>(
    req: &mut Request<T>,
    endpoint: &str,
    method: &str,
    project_id: &str,
) -> Result<(), String> {
    match load_signing_key_for_endpoint(endpoint) {
        Ok(None) => Ok(()),
        Ok(Some(sk)) => {
            attach_auth_metadata(req, &sk, method, project_id, "").map_err(|e| e.to_string())
        }
        Err(e) => Err(e),
    }
}

fn map_host_stats(r: &HostStatsResponse) -> Value {
    json!({
        "disk_free_bytes": r.disk_free_bytes,
        "disk_total_bytes": r.disk_total_bytes,
        "disk_mount_path": r.disk_mount_path,
        "memory_used_bytes": r.memory_used_bytes,
        "memory_total_bytes": r.memory_total_bytes,
        "cpu_usage_percent": r.cpu_usage_percent,
        "load_average_1m": r.load_average_1m,
        "load_average_5m": r.load_average_5m,
        "load_average_15m": r.load_average_15m,
        "temperature_current_celsius": r.temperature_current_celsius,
        "temperature_avg_celsius": r.temperature_avg_celsius,
        "process_count": r.process_count,
        "disk_mounts": r.disk_mounts.iter().map(|m| json!({
            "path": m.path,
            "total_bytes": m.total_bytes,
            "free_bytes": m.free_bytes,
        })).collect::<Vec<_>>(),
        "network_interfaces": r.network_interfaces.iter().map(|n| json!({
            "name": n.name,
            "rx_bytes_per_s": n.rx_bytes_per_s,
            "tx_bytes_per_s": n.tx_bytes_per_s,
            "rx_errors": n.rx_errors,
            "tx_errors": n.tx_errors,
        })).collect::<Vec<_>>(),
        "log_tail": r.log_tail.iter().map(|l| json!({
            "ts_ms": l.ts_ms,
            "level": l.level,
            "message": l.message,
        })).collect::<Vec<_>>(),
    })
}

fn map_cpu_detail(p: &CpuDetailProto) -> Value {
    json!({
        "ts_ms": p.ts_ms,
        "loadavg": p.loadavg.as_ref().map(|l| json!({
            "m1": l.m1, "m5": l.m5, "m15": l.m15,
        })),
        "times": p.times.as_ref().map(|t| json!({
            "user_ms": t.user_ms,
            "system_ms": t.system_ms,
            "idle_ms": t.idle_ms,
        })),
        "top_processes": p.top_processes.iter().map(|x| json!({
            "pid": x.pid,
            "name": x.name,
            "cpu_percent": x.cpu_percent,
        })).collect::<Vec<_>>(),
        "series_hint": p.series_hint.as_ref().map(|s| json!({
            "available_ranges": s.available_ranges,
        })),
    })
}

fn map_memory_detail(p: &MemoryDetailProto) -> Value {
    json!({
        "ts_ms": p.ts_ms,
        "memory": p.memory.as_ref().map(|m| json!({
            "total_bytes": m.total_bytes,
            "used_bytes": m.used_bytes,
            "available_bytes": m.available_bytes,
            "cached_bytes": m.cached_bytes,
            "buffers_bytes": m.buffers_bytes,
            "swap_total_bytes": m.swap_total_bytes,
            "swap_used_bytes": m.swap_used_bytes,
        })),
        "top_processes": p.top_processes.iter().map(|x| json!({
            "pid": x.pid,
            "name": x.name,
            "memory_bytes": x.memory_bytes,
        })).collect::<Vec<_>>(),
    })
}

fn map_disk_detail(p: &DiskDetailProto) -> Value {
    json!({
        "ts_ms": p.ts_ms,
        "mounts": p.mounts.iter().map(|m| json!({
            "path": m.path,
            "total_bytes": m.total_bytes,
            "free_bytes": m.free_bytes,
        })).collect::<Vec<_>>(),
        "io": p.io.as_ref().map(|io| json!({ "note": io.note })),
        "top_processes": p.top_processes.iter().map(|x| json!({
            "pid": x.pid,
            "name": x.name,
            "read_bytes": x.read_bytes,
            "write_bytes": x.write_bytes,
        })).collect::<Vec<_>>(),
    })
}

fn map_network_detail(p: &NetworkDetailProto) -> Value {
    json!({
        "ts_ms": p.ts_ms,
        "interfaces": p.interfaces.iter().map(|n| json!({
            "name": n.name,
            "rx_bytes_per_s": n.rx_bytes_per_s,
            "tx_bytes_per_s": n.tx_bytes_per_s,
            "rx_errors": n.rx_errors,
            "tx_errors": n.tx_errors,
        })).collect::<Vec<_>>(),
        "connections_note": p.connections_note,
    })
}

fn map_processes_detail(p: &ProcessesDetailProto) -> Value {
    json!({
        "ts_ms": p.ts_ms,
        "processes": p.processes.iter().map(|x| json!({
            "pid": x.pid,
            "name": x.name,
            "cpu_percent": x.cpu_percent,
            "memory_bytes": x.memory_bytes,
        })).collect::<Vec<_>>(),
        "total": p.total,
    })
}

fn map_detail_response(r: &HostStatsDetailResponse) -> Result<Value, String> {
    let d = r
        .detail
        .as_ref()
        .ok_or_else(|| "empty detail response".to_string())?;
    Ok(match d {
        DetailOneof::Cpu(p) => json!({ "kind": "cpu", "data": map_cpu_detail(p) }),
        DetailOneof::Memory(p) => json!({ "kind": "memory", "data": map_memory_detail(p) }),
        DetailOneof::Disk(p) => json!({ "kind": "disk", "data": map_disk_detail(p) }),
        DetailOneof::Network(p) => json!({ "kind": "network", "data": map_network_detail(p) }),
        DetailOneof::Processes(p) => json!({ "kind": "processes", "data": map_processes_detail(p) }),
    })
}

/// JSON string matching server-stack `HostStatsView` shape (snake_case keys).
pub fn fetch_host_stats_json() -> Result<String, String> {
    let endpoint = load_endpoint().ok_or_else(|| "no saved gRPC endpoint".to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let pid = load_project_id();
        let mut req = Request::new(HostStatsRequest {
            project_id: pid.clone(),
        });
        attach_if_paired(&mut req, &endpoint, "GetHostStats", &pid)?;
        let r = client
            .get_host_stats(req)
            .await
            .map_err(|e| format!("GetHostStats failed: {e}"))?
            .into_inner();
        serde_json::to_string(&map_host_stats(&r)).map_err(|e| e.to_string())
    })
}

/// `kind` is [`HostStatsDetailKind`] discriminant (1=cpu … 5=processes).
pub fn fetch_host_stats_detail_json(
    kind: i32,
    top: u32,
    q: String,
    limit: u32,
) -> Result<String, String> {
    if kind == HostStatsDetailKind::Unspecified as i32 {
        return Err("kind is required".into());
    }
    let endpoint = load_endpoint().ok_or_else(|| "no saved gRPC endpoint".to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let pid = load_project_id();
        let mut req = Request::new(HostStatsDetailRequest {
            project_id: pid.clone(),
            kind,
            top,
            q,
            limit,
        });
        attach_if_paired(&mut req, &endpoint, "GetHostStatsDetail", &pid)?;
        let r = client
            .get_host_stats_detail(req)
            .await
            .map_err(|e| format!("GetHostStatsDetail failed: {e}"))?
            .into_inner();
        let v = map_detail_response(&r)?;
        serde_json::to_string(&v).map_err(|e| e.to_string())
    })
}
