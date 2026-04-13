//! Managed proxy session: policy encoding, schedule checks, token hashing, limit checks.

use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;
use deploy_proto::deploy::ProxyConnectionPolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tonic::Status;

pub fn hash_session_token_hex(token: &str) -> String {
    let d = Sha256::digest(token.as_bytes());
    hex::encode(d)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccessWindow {
    pub days: Vec<u32>,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccessSchedule {
    pub timezone: String,
    pub windows: Vec<StoredAccessWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPolicy {
    pub max_session_duration_sec: Option<u64>,
    pub traffic_total_bytes: Option<u64>,
    pub traffic_bytes_in_limit: Option<u64>,
    pub traffic_bytes_out_limit: Option<u64>,
    pub active_idle_timeout_sec: Option<u32>,
    pub access_schedule: Option<StoredAccessSchedule>,
    /// When true, session row uses far-future `expires_at`; UI may show as unlimited (-1).
    #[serde(default)]
    pub never_expires: bool,
    /// When true, `max_session_duration_sec` is enforced against cumulative `active_ms`, not wall-clock since `first_open_at`.
    #[serde(default)]
    pub limit_duration_by_active_time: bool,
    /// Max concurrent tunnels per session; `None` = use `DEPLOY_DEFAULT_MAX_DEVICES_PER_SESSION` on server.
    #[serde(default)]
    pub max_concurrent_devices_per_session: Option<u32>,
}

impl StoredPolicy {
    pub fn from_proto(p: &ProxyConnectionPolicy) -> Self {
        Self {
            max_session_duration_sec: p.max_session_duration_sec,
            traffic_total_bytes: p.traffic_total_bytes,
            traffic_bytes_in_limit: p.traffic_bytes_in_limit,
            traffic_bytes_out_limit: p.traffic_bytes_out_limit,
            active_idle_timeout_sec: p.active_idle_timeout_sec,
            access_schedule: p.access_schedule.as_ref().map(|s| StoredAccessSchedule {
                timezone: s.timezone.clone(),
                windows: s
                    .windows
                    .iter()
                    .map(|w| StoredAccessWindow {
                        days: w.days.clone(),
                        start: w.start.clone(),
                        end: w.end.clone(),
                    })
                    .collect(),
            }),
            never_expires: p.never_expires.unwrap_or(false),
            limit_duration_by_active_time: p.limit_duration_by_active_time.unwrap_or(false),
            max_concurrent_devices_per_session: p.max_concurrent_devices_per_session,
        }
    }

    /// Default when policy omits the field (`DEPLOY_DEFAULT_MAX_DEVICES_PER_SESSION`, default 10).
    #[must_use]
    pub fn default_max_devices_per_session_from_env() -> u32 {
        std::env::var("DEPLOY_DEFAULT_MAX_DEVICES_PER_SESSION")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(10)
    }

    /// Effective cap for Redis per-session tunnel limit. `0` = unlimited (no SCARD check).
    #[must_use]
    pub fn effective_max_concurrent_devices_per_session(&self) -> u32 {
        let mut v = match self.max_concurrent_devices_per_session {
            None => Self::default_max_devices_per_session_from_env(),
            Some(0) => 0,
            Some(n) => n,
        };
        if let Ok(cap_s) = std::env::var("DEPLOY_MAX_DEVICES_PER_SESSION_CAP") {
            if let Ok(cap) = cap_s.parse::<u32>() {
                if cap > 0 {
                    v = v.min(cap);
                }
            }
        }
        v
    }

    pub fn idle_timeout_sec(&self) -> u64 {
        self.active_idle_timeout_sec.unwrap_or(60).max(1) as u64
    }
}

pub fn is_schedule_allowed_stored(
    policy: &StoredPolicy,
    now: DateTime<Utc>,
) -> Result<bool, Status> {
    let Some(s) = policy.access_schedule.as_ref() else {
        return Ok(true);
    };
    if s.windows.is_empty() {
        return Ok(true);
    }
    let tz: Tz = if s.timezone.is_empty() {
        chrono_tz::UTC
    } else {
        s.timezone
            .parse()
            .map_err(|_| Status::invalid_argument("invalid timezone"))?
    };
    let local = now.with_timezone(&tz);
    let weekday = match local.weekday() {
        Weekday::Mon => 1u32,
        Weekday::Tue => 2,
        Weekday::Wed => 3,
        Weekday::Thu => 4,
        Weekday::Fri => 5,
        Weekday::Sat => 6,
        Weekday::Sun => 7,
    };
    for w in &s.windows {
        if !w.days.contains(&weekday) {
            continue;
        }
        if window_contains(&local, &w.start, &w.end)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn policy_json_from_proto(p: &ProxyConnectionPolicy) -> Result<String, Status> {
    serde_json::to_string(&StoredPolicy::from_proto(p)).map_err(|e| Status::internal(e.to_string()))
}

pub fn parse_policy_json(s: &str) -> Result<StoredPolicy, Status> {
    serde_json::from_str(s).map_err(|e| Status::internal(format!("policy json: {e}")))
}

pub fn expires_at_from_policy(
    policy: &ProxyConnectionPolicy,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    if policy.never_expires.unwrap_or(false) {
        return now + Duration::days(36500);
    }
    match policy.max_session_duration_sec {
        None | Some(0) => now + Duration::days(36500),
        Some(sec) => now + Duration::seconds(sec as i64),
    }
}

/// `-1` in API responses when the invitation has no calendar cap (`-1` / omitted max duration) or explicit `never_expires`.
pub fn client_expires_at_unix_ms(policy: &ProxyConnectionPolicy, expires_at: DateTime<Utc>) -> i64 {
    if policy.never_expires.unwrap_or(false) {
        return -1;
    }
    if policy.max_session_duration_sec.is_none() {
        return -1;
    }
    expires_at.timestamp_millis()
}

pub fn is_schedule_allowed(
    policy: &ProxyConnectionPolicy,
    now: DateTime<Utc>,
) -> Result<bool, Status> {
    let Some(s) = policy.access_schedule.as_ref() else {
        return Ok(true);
    };
    if s.windows.is_empty() {
        return Ok(true);
    }
    let tz: Tz = if s.timezone.is_empty() {
        chrono_tz::UTC
    } else {
        s.timezone
            .parse()
            .map_err(|_| Status::invalid_argument("invalid timezone"))?
    };
    let local = now.with_timezone(&tz);
    let weekday = match local.weekday() {
        Weekday::Mon => 1u32,
        Weekday::Tue => 2,
        Weekday::Wed => 3,
        Weekday::Thu => 4,
        Weekday::Fri => 5,
        Weekday::Sat => 6,
        Weekday::Sun => 7,
    };
    for w in &s.windows {
        if !w.days.contains(&weekday) {
            continue;
        }
        if window_contains(&local, &w.start, &w.end)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_hhmm(s: &str) -> Result<NaiveTime, Status> {
    let s = s.trim();
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(Status::invalid_argument("time must be HH:MM"));
    }
    let h: u32 = parts[0]
        .parse()
        .map_err(|_| Status::invalid_argument("bad hour"))?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| Status::invalid_argument("bad minute"))?;
    NaiveTime::from_hms_opt(h, m, 0).ok_or_else(|| Status::invalid_argument("invalid time"))
}

fn window_contains(
    local: &chrono::DateTime<Tz>,
    start: &str,
    end: &str,
) -> Result<bool, Status> {
    let t = local.time();
    let st = parse_hhmm(start)?;
    let en = parse_hhmm(end)?;
    if st <= en {
        Ok(t >= st && t <= en)
    } else {
        Ok(t >= st || t <= en)
    }
}

pub fn check_traffic_limits(
    policy: &StoredPolicy,
    bytes_in: u64,
    bytes_out: u64,
) -> Result<(), Status> {
    if let Some(lim) = policy.traffic_bytes_in_limit {
        if lim > 0 && bytes_in > lim {
            return Err(Status::resource_exhausted("proxy session inbound traffic limit"));
        }
    }
    if let Some(lim) = policy.traffic_bytes_out_limit {
        if lim > 0 && bytes_out > lim {
            return Err(Status::resource_exhausted("proxy session outbound traffic limit"));
        }
    }
    if let Some(lim) = policy.traffic_total_bytes {
        if lim > 0 && bytes_in.saturating_add(bytes_out) > lim {
            return Err(Status::resource_exhausted("proxy session total traffic limit"));
        }
    }
    Ok(())
}

pub fn max_duration_exceeded(
    policy: &StoredPolicy,
    first_open_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    if policy.limit_duration_by_active_time {
        return false;
    }
    let Some(sec) = policy.max_session_duration_sec.filter(|s| *s > 0) else {
        return false;
    };
    let Some(t0) = first_open_at else {
        return false;
    };
    (now - t0).num_seconds() > sec as i64
}

/// Upper bound in milliseconds for cumulative active time when [`StoredPolicy::limit_duration_by_active_time`] is set and duration is positive.
pub fn active_time_budget_ms_max(policy: &StoredPolicy) -> Option<u64> {
    if !policy.limit_duration_by_active_time {
        return None;
    }
    let sec = policy.max_session_duration_sec.filter(|s| *s > 0)?;
    Some(sec.saturating_mul(1000))
}

/// Whether cumulative active time (persisted + in-flight) meets or exceeds the budget.
pub fn active_time_budget_exceeded(
    policy: &StoredPolicy,
    active_ms_total: u64,
    inflight_ms: u64,
) -> bool {
    let Some(budget) = active_time_budget_ms_max(policy) else {
        return false;
    };
    active_ms_total.saturating_add(inflight_ms) >= budget
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// Same idle-aware rule as `ActiveTracker` in deploy_service / wire_relay: each gap between
    /// consecutive bumps adds only if `gap_ms <= idle_ms`.
    fn accum_ms_from_bump_gaps_ms(idle_ms: u64, gaps_between_bumps_ms: &[u64]) -> u64 {
        let mut acc = 0u64;
        for &g in gaps_between_bumps_ms {
            if g <= idle_ms {
                acc = acc.saturating_add(g);
            }
        }
        acc
    }

    fn policy_duration_10s_wall_clock() -> StoredPolicy {
        StoredPolicy {
            max_session_duration_sec: Some(10),
            traffic_total_bytes: None,
            traffic_bytes_in_limit: None,
            traffic_bytes_out_limit: None,
            active_idle_timeout_sec: Some(60),
            access_schedule: None,
            never_expires: false,
            limit_duration_by_active_time: false,
            max_concurrent_devices_per_session: None,
        }
    }

    fn policy_duration_10s_active_only() -> StoredPolicy {
        StoredPolicy {
            max_session_duration_sec: Some(10),
            traffic_total_bytes: None,
            traffic_bytes_in_limit: None,
            traffic_bytes_out_limit: None,
            active_idle_timeout_sec: Some(60),
            access_schedule: None,
            never_expires: false,
            limit_duration_by_active_time: true,
            max_concurrent_devices_per_session: None,
        }
    }

    #[test]
    fn wall_clock_10s_exceeded_after_11s_since_first_open() {
        let p = policy_duration_10s_wall_clock();
        let now = Utc::now();
        let first_open = now - Duration::seconds(11);
        assert!(
            max_duration_exceeded(&p, Some(first_open), now),
            "without limit_duration_by_active_time, duration is wall-clock since first_open_at"
        );
    }

    #[test]
    fn wall_clock_10s_not_exceeded_after_5s_since_first_open() {
        let p = policy_duration_10s_wall_clock();
        let now = Utc::now();
        let first_open = now - Duration::seconds(5);
        assert!(!max_duration_exceeded(&p, Some(first_open), now));
    }

    #[test]
    fn wall_clock_exceeded_long_after_first_open_regardless_of_active_ms() {
        let p = policy_duration_10s_wall_clock();
        let now = Utc::now();
        let first_open = now - Duration::seconds(100);
        // Wall-clock cap: not using limit_duration_by_active_time
        assert!(max_duration_exceeded(&p, Some(first_open), now));
    }

    #[test]
    fn active_time_flag_disables_wall_clock_check() {
        let p = policy_duration_10s_active_only();
        let now = Utc::now();
        let first_open = now - Duration::hours(24);
        assert!(
            !max_duration_exceeded(&p, Some(first_open), now),
            "with limit_duration_by_active_time, wall-clock max_duration_exceeded is not used"
        );
    }

    #[test]
    fn active_budget_10s_exceeded_when_persisted_plus_inflight_ge_10000ms() {
        let p = policy_duration_10s_active_only();
        assert!(!active_time_budget_exceeded(&p, 9000, 999));
        assert!(active_time_budget_exceeded(&p, 9000, 1000));
        assert!(active_time_budget_exceeded(&p, 10000, 0));
    }

    #[test]
    fn traffic_inbound_limit_blocks() {
        let mut p = policy_duration_10s_wall_clock();
        p.traffic_bytes_in_limit = Some(1000);
        assert!(check_traffic_limits(&p, 1001, 0).is_err());
        assert!(check_traffic_limits(&p, 1000, 0).is_ok());
    }

    #[test]
    fn traffic_outbound_limit_blocks() {
        let mut p = policy_duration_10s_wall_clock();
        p.traffic_bytes_out_limit = Some(500);
        assert!(check_traffic_limits(&p, 0, 501).is_err());
        assert!(check_traffic_limits(&p, 0, 500).is_ok());
    }

    #[test]
    fn traffic_total_limit_blocks() {
        let mut p = policy_duration_10s_wall_clock();
        p.traffic_total_bytes = Some(2000);
        assert!(check_traffic_limits(&p, 1000, 1001).is_err());
        assert!(check_traffic_limits(&p, 1000, 1000).is_ok());
    }

    /// Simulate: 4s activity (bumps 4s apart within idle window), disconnect (no bumps 10s — does not add),
    /// reconnect: another 5s of in-tunnel gaps → persisted 4s + inflight 5s = 9s < 10s budget.
    #[test]
    fn active_mode_disconnect_pause_does_not_add_idle_wall_time() {
        let idle_ms = 60_000u64;
        let tunnel1_gaps_ms = &[4000u64];
        let ms_tunnel1 = accum_ms_from_bump_gaps_ms(idle_ms, tunnel1_gaps_ms);
        assert_eq!(ms_tunnel1, 4000, "single 4s interval between bumps counts");

        let persisted = ms_tunnel1;
        // No proxy traffic while disconnected — nothing added here.

        let tunnel2_gaps_ms = &[5000u64];
        let inflight = accum_ms_from_bump_gaps_ms(idle_ms, tunnel2_gaps_ms);
        assert_eq!(inflight, 5000);

        let p = policy_duration_10s_active_only();
        assert!(
            !active_time_budget_exceeded(&p, persisted, inflight),
            "9s total active < 10s budget"
        );
        assert!(active_time_budget_exceeded(&p, persisted, inflight + 1000));
    }

    /// Within one tunnel, gap between bumps ≤ idle counts (idle-aware "active" meter).
    #[test]
    fn active_mode_idle_gap_under_idle_timeout_counts_between_bumps() {
        let idle_ms = 60_000;
        let gaps = &[3000u64, 10_000];
        let acc = accum_ms_from_bump_gaps_ms(idle_ms, gaps);
        assert_eq!(acc, 13_000, "10s pause between bytes still counts when <= idle window");
    }

    #[test]
    fn active_mode_gap_over_idle_does_not_add() {
        let idle_ms = 5_000;
        let gaps = &[6_000];
        let acc = accum_ms_from_bump_gaps_ms(idle_ms, gaps);
        assert_eq!(acc, 0, "gap longer than idle adds nothing (same as server ActiveTracker)");
    }
}
