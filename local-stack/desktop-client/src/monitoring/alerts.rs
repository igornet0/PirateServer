//! Simple threshold alerts (CPU high, disk low).

use std::sync::atomic::{AtomicBool, Ordering};

use super::types::MonitoringOverview;

pub struct AlertConfig {
    pub enabled: AtomicBool,
    pub economy_mode: AtomicBool,
    pub cpu_high_pct: f32,
    pub disk_low_free_bytes: u64,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            economy_mode: AtomicBool::new(false),
            cpu_high_pct: 90.0,
            disk_low_free_bytes: 512 * 1024 * 1024,
        }
    }
}

impl AlertConfig {
    pub fn evaluate(&self, o: &MonitoringOverview) -> Vec<String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut out = Vec::new();
        if o.cpu.usage_percent >= self.cpu_high_pct {
            out.push(format!(
                "cpu_high: {:.1}% >= {:.1}%",
                o.cpu.usage_percent, self.cpu_high_pct
            ));
        }
        for m in &o.disk.mounts {
            if m.free_bytes < self.disk_low_free_bytes {
                out.push(format!(
                    "disk_low: {} free {} bytes",
                    m.path, m.free_bytes
                ));
            }
        }
        out
    }
}
