//! Local resource scores (0–1000) written to `server_resource_benchmark` for dashboard display.

use deploy_db::DbStore;
use std::time::{Duration, Instant};

fn bench_cpu_score() -> i32 {
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < Duration::from_millis(50) {
        n = n.wrapping_add(1);
    }
    ((n / 5000).min(1000)) as i32
}

fn bench_ram_score() -> i32 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let gb = sys.total_memory() / (1024 * 1024 * 1024);
    ((gb as i32).saturating_mul(50)).min(1000).max(0)
}

fn bench_storage_score() -> std::io::Result<i32> {
    let buf = vec![0xABu8; 4 * 1024 * 1024];
    let path = std::env::temp_dir().join(format!("pirate-bench-{}", std::process::id()));
    let start = Instant::now();
    std::fs::write(&path, &buf)?;
    let _ = std::fs::read(&path)?;
    let _ = std::fs::remove_file(&path);
    let ms = start.elapsed().as_millis().max(1);
    let score = ((4u128 * 1000) / ms as u128).min(1000) as i32;
    Ok(score.max(1))
}

fn bench_gpu_score_optional() -> Option<i32> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout);
    if name.trim().is_empty() {
        return None;
    }
    Some(650)
}

/// Run quick benchmarks and insert one row into the metadata DB.
pub async fn run_resource_benchmark(db: &DbStore) -> Result<(), Box<dyn std::error::Error>> {
    let cpu_score = bench_cpu_score();
    let ram_score = bench_ram_score();
    let storage_score = bench_storage_score()?;
    let gpu_score = bench_gpu_score_optional();
    let raw = serde_json::json!({
        "cpu_score": cpu_score,
        "ram_score": ram_score,
        "storage_score": storage_score,
        "gpu_score": gpu_score,
        "note": "heuristic scores 0-1000; not comparable across machines"
    });
    db.insert_server_resource_benchmark(
        cpu_score,
        ram_score,
        storage_score,
        gpu_score,
        &raw.to_string(),
    )
    .await?;
    Ok(())
}
