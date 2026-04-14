//! Resource limits and execution policy for native workloads (server-side).
//!
//! Full OS sandboxing (cgroups v2, namespaces) is platform-specific; callers should
//! apply these hints when spawning processes.

use serde::{Deserialize, Serialize};

/// Declared limits from `pirate.toml` / server policy (enforced best-effort).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxLimits {
    /// Percent of one CPU core (100 = 1 core cap when enforced via OS).
    pub cpu_limit_percent: u32,
    pub memory_limit_mb: u32,
    /// Wall-clock timeout for startup health (ms).
    pub deploy_timeout_ms: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            cpu_limit_percent: 100,
            memory_limit_mb: 512,
            deploy_timeout_ms: 120_000,
        }
    }
}

/// Validate tarball paths before unpack (caller already strips `..`).
pub fn is_safe_relative_path(p: &str) -> bool {
    !p.is_empty() && !p.contains("..") && !p.starts_with('/')
}
