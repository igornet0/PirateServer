//! SQLite persistence for monitoring samples (shared `pirate_desktop.db`).

use crate::desktop_store;

use super::types::MonitoringOverview;

const SAMPLE_RETENTION_DAYS: i64 = 7;
const SAMPLE_MAX_ROWS: i64 = 50_000;

pub fn append_sample(o: &MonitoringOverview) -> Result<(), rusqlite::Error> {
    let c = desktop_store::open()?;
    c.execute(
        "INSERT OR REPLACE INTO samples (ts_ms, cpu, mem_used) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            o.ts_ms,
            o.cpu.usage_percent as f64,
            o.memory.used_bytes as i64
        ],
    )?;
    prune_samples(&c)?;
    Ok(())
}

fn prune_samples(c: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    let cutoff = chrono::Utc::now().timestamp_millis() - SAMPLE_RETENTION_DAYS * 86_400_000;
    c.execute("DELETE FROM samples WHERE ts_ms < ?1", rusqlite::params![cutoff])?;
    let count: i64 = c.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))?;
    if count > SAMPLE_MAX_ROWS {
        let excess = count - SAMPLE_MAX_ROWS;
        c.execute(
            "DELETE FROM samples WHERE ts_ms IN (
                SELECT ts_ms FROM samples ORDER BY ts_ms ASC LIMIT ?1
            )",
            rusqlite::params![excess],
        )?;
    }
    Ok(())
}

/// Baseline metric: on-disk samples row count.
pub fn sample_row_count() -> Result<i64, String> {
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}
