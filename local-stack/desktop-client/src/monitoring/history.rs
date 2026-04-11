//! In-memory ring buffers for chart series (optional SQLite persistence behind feature).

use std::collections::{HashMap, VecDeque};

use super::types::{MonitoringOverview, SeriesPoint, SeriesResponse};

const DEFAULT_CAP: usize = 3600;

pub struct HistoryBuffer {
    cap: usize,
    series: HashMap<String, VecDeque<(i64, f64)>>,
}

impl HistoryBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(60),
            series: HashMap::new(),
        }
    }

    pub fn default_cap() -> usize {
        DEFAULT_CAP
    }

    fn push_metric(&mut self, key: &str, ts_ms: i64, value: f64) {
        let dq = self.series.entry(key.to_string()).or_insert_with(|| VecDeque::with_capacity(self.cap));
        dq.push_back((ts_ms, value));
        while dq.len() > self.cap {
            dq.pop_front();
        }
    }

    pub fn record_overview(&mut self, o: &MonitoringOverview) {
        let ts = o.ts_ms;
        self.push_metric("cpu", ts, o.cpu.usage_percent as f64);
        self.push_metric("memory_used", ts, o.memory.used_bytes as f64);
        let net_rx: f64 = o
            .network
            .interfaces
            .iter()
            .map(|i| i.rx_bytes_per_s)
            .sum();
        let net_tx: f64 = o
            .network
            .interfaces
            .iter()
            .map(|i| i.tx_bytes_per_s)
            .sum();
        self.push_metric("net_rx", ts, net_rx);
        self.push_metric("net_tx", ts, net_tx);
    }

    pub fn series(&self, metric: &str, range_ms: i64, step_ms: u64) -> SeriesResponse {
        let now = chrono::Utc::now().timestamp_millis();
        let start = now.saturating_sub(range_ms);
        let points: Vec<SeriesPoint> = self
            .series
            .get(metric)
            .map(|dq| {
                dq.iter()
                    .filter(|(ts, _)| *ts >= start)
                    .map(|(ts, v)| SeriesPoint {
                        ts_ms: *ts,
                        value: *v,
                    })
                    .collect()
            })
            .unwrap_or_default();

        SeriesResponse {
            metric: metric.to_string(),
            step_ms,
            points,
        }
    }
}
