//! Wall-clock helpers for challenge freshness.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in milliseconds.
pub fn unix_timestamp_ms_now() -> Result<i64, std::time::SystemTimeError> {
    let d = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(d.as_millis() as i64)
}
