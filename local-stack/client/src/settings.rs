//! `settings.json`: boards, bypass, routing; optional hot reload via `notify`.

#![allow(dead_code)]

use crate::config::settings_path;
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
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BoardConfig {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub bypass: Vec<String>,
    /// Opaque session token from server `CreateConnection` (optional).
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub max_concurrent_tunnels: Option<usize>,
    /// Optional `vless://` / `vmess://` / `trojan://` — tunnel uses wire protocol over gRPC (Pirate-only).
    #[serde(default)]
    pub wire_subscription_uri: Option<String>,
}

fn default_true() -> bool {
    true
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
    pub loaded_at: Instant,
}

impl SettingsSnapshot {
    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            data: SettingsFile::default(),
            loaded_at: Instant::now(),
        }
    }
}

pub fn default_settings_path() -> PathBuf {
    settings_path().unwrap_or_else(|| PathBuf::from("settings.json"))
}

/// Load or create empty settings.
pub fn load_settings_from_path(path: &Path) -> Result<SettingsFile, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(SettingsFile::default());
    }
    let text = std::fs::read_to_string(path)?;
    let s: SettingsFile = serde_json::from_str(&text)?;
    Ok(s)
}

pub fn init_global_settings(path: PathBuf) -> Result<Arc<RwLock<SettingsSnapshot>>, Box<dyn std::error::Error>> {
    let data = load_settings_from_path(&path)?;
    let snap = Arc::new(RwLock::new(SettingsSnapshot {
        path: path.clone(),
        data,
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
                    let mut g = snap2.write();
                    g.data = data;
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
