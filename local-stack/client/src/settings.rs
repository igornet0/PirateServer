//! `settings.json`: boards, bypass, routing; optional hot reload via `notify`.

#![allow(dead_code)]

use crate::config::settings_path;
pub use crate::default_rules::DefaultRulesPaths;
use crate::default_rules::{compile_default_rules, CompiledDefaultRules};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

static SNAPSHOT: OnceCell<Arc<RwLock<SettingsSnapshot>>> = OnceCell::new();

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SettingsFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub global: GlobalSettings,
    #[serde(default)]
    pub default_board: String,
    #[serde(default)]
    pub boards: HashMap<String, BoardConfig>,
    #[serde(default)]
    pub routing: Vec<RoutingRule>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GlobalSettings {
    #[serde(default)]
    pub bypass: Vec<String>,
    /// Optional paths to JSON rule bundles (`_block`, `_pass`, `_our`); see `default_rules`.
    #[serde(default)]
    pub default_rules: DefaultRulesPaths,
    /// `merged` | `bundles` | `board` — see [`TrafficRuleSource`].
    #[serde(default)]
    pub traffic_rule_source: String,
}

/// Where host-rule lists are taken from: JSON bundles, inline board lists, or both (legacy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrafficRuleSource {
    /// Union of `global.default_rules` files and per-board lists (original behavior).
    #[default]
    Merged,
    /// Only compiled `default_rules` JSON; ignore board `anti_adw` / `ru_block` / `bypass` lists.
    Bundles,
    /// Only board inline lists + `global.bypass`; ignore compiled JSON rule sets.
    Board,
}

impl TrafficRuleSource {
    /// Parse from `settings.json` string; unknown or empty → `Merged`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "bundles" => Self::Bundles,
            "board" => Self::Board,
            _ => Self::Merged,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Bundles => "bundles",
            Self::Board => "board",
        }
    }
}

impl GlobalSettings {
    pub fn traffic_rule_source(&self) -> TrafficRuleSource {
        TrafficRuleSource::parse(&self.traffic_rule_source)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BoardConfig {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass: Vec<String>,
    /// Opaque session token from server `CreateConnection` (optional).
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub max_concurrent_tunnels: Option<usize>,
    /// Server-side tunnel admission priority (higher = scheduled first when at concurrency limit). Default 0.
    #[serde(default)]
    pub tunnel_priority: Option<i32>,
    /// Optional `vless://` / `vmess://` / `trojan://` — tunnel uses wire protocol over gRPC (Pirate-only).
    #[serde(default)]
    pub wire_subscription_uri: Option<String>,
    /// When true: padding/jitter hooks, browser-like gRPC metadata, strict TLS profile selection.
    #[serde(default)]
    pub stealth_mode: bool,
    #[serde(default)]
    pub stealth: Option<StealthConfig>,
    /// TLS SNI hostname (can differ from gRPC endpoint host for fronting).
    #[serde(default)]
    pub grpc_tls_sni: Option<String>,
    /// Built-in profile name for rustls `ClientConfig` (e.g. `modern`, `compat`).
    #[serde(default)]
    pub tls_profile: Option<String>,
    #[serde(default)]
    pub experimental_http3: bool,
    /// `quic` | `tcp` | `auto` — proxy data-plane transport (`auto` tries QUIC then gRPC+TCP relay).
    #[serde(default)]
    pub transport_mode: Option<String>,
    /// Override QUIC UDP port when server omits it (default 7844).
    #[serde(default)]
    pub quic_port: Option<u16>,
    /// Optional pinned TLS certificate SHA-256 (hex) for QUIC (rustls).
    #[serde(default)]
    pub quic_tls_cert_sha256: Option<String>,
    /// Accept any server certificate for QUIC (default true for self-signed server certs).
    #[serde(default = "default_true")]
    pub quic_tls_insecure: bool,
    /// HTTP/2 keep-alive interval for gRPC channel (seconds); `None` uses transport defaults.
    #[serde(default)]
    pub grpc_keep_alive_interval_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StealthConfig {
    /// Inclusive range for random delay before opening `ProxyTunnel` (milliseconds).
    #[serde(default)]
    pub jitter_ms_min: u64,
    #[serde(default)]
    pub jitter_ms_max: u64,
}

fn default_true() -> bool {
    true
}

/// Omit `false` bools in JSON (serde: skip when this returns true).
fn skip_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoutingRule {
    #[serde(default)]
    pub suffix: String,
    pub board: String,
}

#[derive(Clone)]
pub struct SettingsSnapshot {
    pub path: PathBuf,
    pub data: SettingsFile,
    pub compiled_default_rules: Option<CompiledDefaultRules>,
    pub loaded_at: Instant,
}

impl SettingsSnapshot {
    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            data: SettingsFile::default(),
            compiled_default_rules: None,
            loaded_at: Instant::now(),
        }
    }
}

pub fn default_settings_path() -> PathBuf {
    settings_path().unwrap_or_else(|| PathBuf::from("settings.json"))
}

/// If only JSON bundles are configured and board lists are unused, set `traffic_rule_source: bundles`
/// and enable the default board so CONNECT works (see `board.enabled` gate in `board.rs`).
fn warn_unknown_traffic_rule_source(g: &GlobalSettings) {
    let t = g.traffic_rule_source.trim();
    if t.is_empty() {
        return;
    }
    let ok = matches!(
        t.to_ascii_lowercase().as_str(),
        "merged" | "bundles" | "board"
    );
    if !ok {
        eprintln!(
            "settings: unknown global.traffic_rule_source {:?}; using merged",
            g.traffic_rule_source
        );
    }
}

pub fn migrate_traffic_rule_settings(data: &mut SettingsFile) {
    if !data.global.traffic_rule_source.trim().is_empty() {
        return;
    }
    let has_bundles = data
        .global
        .default_rules
        .block_json
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        || data
            .global
            .default_rules
            .pass_json
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        || data
            .global
            .default_rules
            .our_json
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    if !has_bundles {
        return;
    }
    let key = if data.default_board.trim().is_empty() {
        "default".to_string()
    } else {
        data.default_board.clone()
    };
    let Some(board) = data.boards.get(&key) else {
        data.global.traffic_rule_source = "bundles".to_string();
        return;
    };
    let lists_empty = board.bypass.is_empty();
    if lists_empty {
        data.global.traffic_rule_source = "bundles".to_string();
        if let Some(b) = data.boards.get_mut(&key) {
            if !b.enabled {
                b.enabled = true;
            }
        }
    }
}

/// Load or create empty settings.
pub fn load_settings_from_path(path: &Path) -> Result<SettingsFile, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(SettingsFile::default());
    }
    let text = std::fs::read_to_string(path)?;
    let mut s: SettingsFile = serde_json::from_str(&text)?;
    migrate_traffic_rule_settings(&mut s);
    warn_unknown_traffic_rule_source(&s.global);
    Ok(s)
}

fn compile_rules_for_path(
    path: &Path,
    data: &SettingsFile,
) -> Result<Option<CompiledDefaultRules>, String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    compile_default_rules(&data.global.default_rules, parent)
}

pub fn init_global_settings(path: PathBuf) -> Result<Arc<RwLock<SettingsSnapshot>>, Box<dyn std::error::Error>> {
    let data = load_settings_from_path(&path)?;
    let compiled_default_rules = compile_rules_for_path(&path, &data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let snap = Arc::new(RwLock::new(SettingsSnapshot {
        path: path.clone(),
        data,
        compiled_default_rules,
        loaded_at: Instant::now(),
    }));
    let _ = SNAPSHOT.set(snap.clone());

    let path2 = path.clone();
    let snap2 = snap.clone();
    std::thread::spawn(move || {
        use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("settings watch: {e}");
                return;
            }
        };
        let watch_path = path2
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        if watcher
            .watch(watch_path, RecursiveMode::NonRecursive)
            .is_err()
        {
            let _ = watcher.watch(&path2, RecursiveMode::NonRecursive);
        }
        let mut last_reload = Instant::now();
        while rx.recv().is_ok() {
            if last_reload.elapsed() < Duration::from_millis(150) {
                std::thread::sleep(Duration::from_millis(150));
            }
            last_reload = Instant::now();
            match load_settings_from_path(&path2) {
                Ok(data) => {
                    let rules = compile_rules_for_path(&path2, &data)
                        .unwrap_or_else(|e| {
                            eprintln!("default rules reload: {e}");
                            None
                        });
                    let mut g = snap2.write();
                    g.data = data;
                    g.compiled_default_rules = rules;
                    g.loaded_at = Instant::now();
                }
                Err(e) => eprintln!("settings reload: {e}"),
            }
        }
    });

    Ok(snap)
}

pub fn global_settings() -> Option<Arc<RwLock<SettingsSnapshot>>> {
    SNAPSHOT.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_config_omits_default_rule_fields_in_json() {
        let mut sf = SettingsFile::default();
        sf.global.traffic_rule_source = "bundles".to_string();
        sf.default_board = "default".to_string();
        sf.boards.insert("default".to_string(), BoardConfig::default());
        let json = serde_json::to_string(&sf).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let board = v["boards"]["default"].as_object().unwrap();
        assert!(!board.contains_key("bypass"));
    }

    #[test]
    fn board_config_round_trip_minimal_json() {
        let raw = r#"{
            "global": {"traffic_rule_source": "bundles"},
            "default_board": "default",
            "boards": {"default": {"enabled": true}}
        }"#;
        let sf: SettingsFile = serde_json::from_str(raw).unwrap();
        assert!(sf.boards["default"].enabled);
        let out = serde_json::to_string(&sf).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let board = v["boards"]["default"].as_object().unwrap();
        assert!(!board.contains_key("anti_adw"));
    }
}
