//! SQLite persistence for monitoring samples (shared `pirate_desktop.db`).

use crate::desktop_store;

use super::types::MonitoringOverview;

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
    Ok(())
}
