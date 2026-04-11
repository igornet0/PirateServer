//! In-memory ring buffers for recent host-stats samples (graphs / series API).

use std::collections::VecDeque;

use crate::types::{SeriesPoint, SeriesResponse};

/// Enough samples for ~24h at a 5s poll interval (17280) plus headroom.
const DEFAULT_MAX_POINTS: usize = 20_000;

fn parse_range_ms(range: &str) -> Option<i64> {
    let r: String = range.chars().filter(|c| !c.is_whitespace()).collect();
    let r = r.to_lowercase();
    match r.as_str() {
        "15m" | "15min" => Some(15 * 60 * 1000),
        "1h" | "60m" | "60min" => Some(60 * 60 * 1000),
        "24h" | "24hr" | "1d" | "1440m" => Some(24 * 60 * 60 * 1000),
        "7d" | "1w" | "week" | "168h" | "168hr" => Some(7 * 24 * 60 * 60 * 1000),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Ring {
    step_hint_ms: u64,
    points: VecDeque<(i64, f64)>,
    max_points: usize,
}

impl Ring {
    fn new(max_points: usize) -> Self {
        Self {
            step_hint_ms: 5000,
            points: VecDeque::with_capacity(max_points.min(1024)),
            max_points,
        }
    }

    fn push(&mut self, ts_ms: i64, value: f64) {
        if let Some(&(last_ts, _)) = self.points.back() {
            let dt = (ts_ms - last_ts).abs() as u64;
            if dt > 0 && dt < 3600_000 {
                self.step_hint_ms = self.step_hint_ms.saturating_mul(9) / 10 + dt / 10;
            }
        }
        self.points.push_back((ts_ms, value));
        while self.points.len() > self.max_points {
            self.points.pop_front();
        }
    }

    fn query(&self, metric: &str, range: &str, now_ms: i64) -> SeriesResponse {
        let window = parse_range_ms(range).unwrap_or(60 * 60 * 1000);
        let cutoff = now_ms.saturating_sub(window);
        let points: Vec<SeriesPoint> = self
            .points
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .map(|(ts, v)| SeriesPoint {
                ts_ms: *ts,
                value: *v,
            })
            .collect();
        SeriesResponse {
            metric: metric.to_string(),
            step_ms: self.step_hint_ms.max(1000),
            points,
        }
    }
}

/// Holds recent samples pushed from control-api after each overview poll.
#[derive(Debug)]
pub struct HostStatsHistory {
    cpu: Ring,
    memory_used: Ring,
    load1: Ring,
    net_rx: Ring,
    net_tx: Ring,
}

impl HostStatsHistory {
    pub fn new(max_points: usize) -> Self {
        let n = max_points.max(64).min(100_000);
        Self {
            cpu: Ring::new(n),
            memory_used: Ring::new(n),
            load1: Ring::new(n),
            net_rx: Ring::new(n),
            net_tx: Ring::new(n),
        }
    }

    pub fn default_new() -> Self {
        Self::new(DEFAULT_MAX_POINTS)
    }

    pub fn record(
        &mut self,
        ts_ms: i64,
        cpu_pct: f32,
        memory_used_bytes: u64,
        load1: f64,
        net_rx_bps: f64,
        net_tx_bps: f64,
    ) {
        self.cpu.push(ts_ms, f64::from(cpu_pct));
        self.memory_used
            .push(ts_ms, memory_used_bytes as f64);
        self.load1.push(ts_ms, load1);
        self.net_rx.push(ts_ms, net_rx_bps);
        self.net_tx.push(ts_ms, net_tx_bps);
    }

    pub fn series(&self, metric: &str, range: &str) -> SeriesResponse {
        let now_ms = chrono::Utc::now().timestamp_millis();
        match metric {
            "cpu" => self.cpu.query("cpu", range, now_ms),
            "memory_used" => self.memory_used.query("memory_used", range, now_ms),
            "load1" => self.load1.query("load1", range, now_ms),
            "net_rx" => self.net_rx.query("net_rx", range, now_ms),
            "net_tx" => self.net_tx.query("net_tx", range, now_ms),
            _ => SeriesResponse {
                metric: metric.to_string(),
                step_ms: 5000,
                points: Vec::new(),
            },
        }
    }
}
