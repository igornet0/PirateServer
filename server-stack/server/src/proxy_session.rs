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
        }
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
    let max_dur = policy.max_session_duration_sec.unwrap_or(0);
    if max_dur > 0 {
        now + Duration::seconds(max_dur as i64)
    } else {
        now + Duration::days(365)
    }
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
    let Some(sec) = policy.max_session_duration_sec.filter(|s| *s > 0) else {
        return false;
    };
    let Some(t0) = first_open_at else {
        return false;
    };
    (now - t0).num_seconds() > sec as i64
}
