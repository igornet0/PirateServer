//! Local HTTP CONNECT proxy (`pirate board`) from the desktop: same `settings.json` as CLI.

use crate::connection::{load_endpoint, load_project_id, load_signing_key_for_endpoint};
use deploy_client::internet_proxy::{
    default_settings_path, init_global_settings, load_settings_from_path, run_board,
    ConnectionManager, DefaultRulesPaths, ProxyTraceBuffer, ProxyTraceEntry, SettingsFile,
    SettingsSnapshot,
};
use deploy_client::internet_proxy::{
    read_rule_bundle_file, serialize_rule_bundle_json, validate_default_rules_json, RuleBundleEdit,
};
use deploy_client::settings::{BoardConfig, TrafficRuleSource};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Relative paths (to `pirate-client/`) for user-edited rule files from the form.
pub const USER_BLOCK_REL: &str = "default-rules/user-block.json";
pub const USER_PASS_REL: &str = "default-rules/user-pass.json";
pub const USER_OUR_REL: &str = "default-rules/user-our.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultRulesBundlesForm {
    pub block: RuleBundleEdit,
    pub pass: RuleBundleEdit,
    pub our: RuleBundleEdit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardRulesForm {
    /// `merged` | `bundles` | `board` — empty means merged.
    #[serde(default)]
    pub traffic_rule_source: String,
    pub default_board: String,
    pub board_id: String,
    pub global_bypass: Vec<String>,
    pub bypass: Vec<String>,
}

static BOARD_TASK: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);
static PROXY_TRACE: Mutex<Option<Arc<ProxyTraceBuffer>>> = Mutex::new(None);
static PROXY_RUNTIME: Mutex<Option<Arc<tokio::runtime::Runtime>>> = Mutex::new(None);
static SETTINGS_SNAPSHOT: Mutex<Option<Arc<parking_lot::RwLock<SettingsSnapshot>>>> =
    Mutex::new(None);
static CONNECTION_POOL: Mutex<Option<Arc<ConnectionManager>>> = Mutex::new(None);

fn ensure_settings_loaded() -> Result<Arc<parking_lot::RwLock<SettingsSnapshot>>, String> {
    let mut g = SETTINGS_SNAPSHOT.lock();
    if let Some(ref s) = *g {
        return Ok(s.clone());
    }
    let path = default_settings_path();
    let snap = init_global_settings(path).map_err(|e| e.to_string())?;
    *g = Some(snap.clone());
    Ok(snap)
}

fn pool() -> Arc<ConnectionManager> {
    let mut g = CONNECTION_POOL.lock();
    g.get_or_insert_with(|| Arc::new(ConnectionManager::new(512)))
        .clone()
}

/// Default listen address for the local CONNECT proxy (same as CLI `pirate board`).
pub fn default_listen_addr() -> &'static str {
    "127.0.0.1:3128"
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InternetProxyStatus {
    pub running: bool,
    pub listen: String,
    pub last_error: Option<String>,
}

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Whether the local proxy task is running.
pub fn internet_proxy_status() -> InternetProxyStatus {
    let running = BOARD_TASK
        .lock()
        .as_ref()
        .map(|h| !h.is_finished())
        .unwrap_or(false);
    InternetProxyStatus {
        running,
        listen: default_listen_addr().to_string(),
        last_error: LAST_ERROR.lock().clone(),
    }
}

/// Start `run_board` on the shared Tokio runtime (spawned task). Requires a saved paired gRPC connection.
pub fn internet_proxy_start(listen: Option<String>) -> Result<(), String> {
    let listen = listen
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_listen_addr().to_string());

    if BOARD_TASK
        .lock()
        .as_ref()
        .map(|h| !h.is_finished())
        .unwrap_or(false)
    {
        return Err("internet proxy already running".into());
    }

    let endpoint = load_endpoint().ok_or_else(|| "save a gRPC connection first".to_string())?;
    let ep = endpoint.trim();
    if !ep.starts_with("http://") && !ep.starts_with("https://") {
        return Err("gRPC URL must start with http:// or https://".into());
    }

    let sk = load_signing_key_for_endpoint(ep)?
        .ok_or_else(|| "pair with the server first (signed gRPC required for ProxyTunnel)".to_string())?;

    let snap = ensure_settings_loaded()?;
    let pool = pool();
    let project = load_project_id();
    let conn_url = endpoint.clone();
    let grpc_ep = endpoint.clone();

    *LAST_ERROR.lock() = None;

    let trace = Arc::new(ProxyTraceBuffer::new(400));
    *PROXY_TRACE.lock() = Some(trace.clone());

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?,
    );
    let handle = {
        let rt2 = rt.clone();
        rt2.spawn(async move {
            if let Err(e) = run_board(
                &listen,
                &grpc_ep,
                &conn_url,
                &sk,
                &project,
                "",
                snap,
                pool,
                None,
                Some(trace),
            )
            .await
            {
                *LAST_ERROR.lock() = Some(e.to_string());
            }
        })
    };

    *PROXY_RUNTIME.lock() = Some(rt);
    *BOARD_TASK.lock() = Some(handle);
    Ok(())
}

/// Abort the background board task and drop the dedicated runtime.
pub fn internet_proxy_stop() -> Result<(), String> {
    if let Some(h) = BOARD_TASK.lock().take() {
        h.abort();
    }
    PROXY_RUNTIME.lock().take();
    Ok(())
}

/// Last captured CONNECT trace lines (ring buffer); empty when proxy never started or buffer missing.
pub fn internet_proxy_logs() -> Vec<ProxyTraceEntry> {
    PROXY_TRACE
        .lock()
        .as_ref()
        .map(|b| b.snapshot())
        .unwrap_or_default()
}

pub fn internet_proxy_logs_clear() {
    if let Some(b) = PROXY_TRACE.lock().as_ref() {
        b.clear();
    }
}

/// Pretty JSON for `settings.json` (same file as CLI `pirate board`).
pub fn load_settings_json() -> Result<String, String> {
    let path = default_settings_path();
    let data = load_settings_from_path(&path).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&data).map_err(|e| e.to_string())
}

/// Replace `settings.json` (validates JSON shape). File watcher reloads compiled rules if proxy was started.
pub fn save_settings_json(text: &str) -> Result<(), String> {
    let parsed: SettingsFile = serde_json::from_str(text).map_err(|e| format!("invalid settings: {e}"))?;
    let path = default_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = serde_json::to_string_pretty(&parsed).map_err(|e| e.to_string())?;
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(())
}

fn default_rules_target_dir() -> Result<PathBuf, String> {
    let base =
        deploy_client::config::config_dir().ok_or_else(|| "no config directory".to_string())?;
    Ok(base.join("default-rules"))
}

/// Write bundled copies of `server-stack/default-rules` next to `settings.json` and return relative paths for `global.default_rules`.
pub fn apply_default_rules_preset(preset: &str) -> Result<DefaultRulesPaths, String> {
    let dir = default_rules_target_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    const ANTI_ADW: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../server-stack/default-rules/anti-adw.json"
    ));
    const RU_BLOCK_DOMAIN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../server-stack/default-rules/ru-block-domain.json"
    ));
    const RU_FULL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../server-stack/default-rules/ru-full.json"
    ));

    std::fs::write(dir.join("anti-adw.json"), ANTI_ADW).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("ru-block-domain.json"), RU_BLOCK_DOMAIN).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("ru-full.json"), RU_FULL).map_err(|e| e.to_string())?;

    // Paths are relative to the parent directory of settings.json (pirate-client/).
    match preset.trim() {
        "combined" | "full" => Ok(DefaultRulesPaths {
            block_json: Some("default-rules/anti-adw.json".into()),
            pass_json: Some("default-rules/ru-full.json".into()),
            our_json: Some("default-rules/ru-block-domain.json".into()),
        }),
        "anti-adw-only" => Ok(DefaultRulesPaths {
            block_json: Some("default-rules/anti-adw.json".into()),
            pass_json: None,
            our_json: None,
        }),
        "ru-full-only" => Ok(DefaultRulesPaths {
            block_json: None,
            pass_json: Some("default-rules/ru-full.json".into()),
            our_json: None,
        }),
        "ru-block-domain-only" => Ok(DefaultRulesPaths {
            block_json: None,
            pass_json: None,
            our_json: Some("default-rules/ru-block-domain.json".into()),
        }),
        _ => Err(format!("unknown preset: {preset}")),
    }
}

/// Write preset JSON files and update `settings.json` `global.default_rules`.
pub fn apply_default_rules_preset_to_disk(preset: &str) -> Result<(), String> {
    let paths = apply_default_rules_preset(preset)?;
    let path = default_settings_path();
    let mut sf = load_settings_from_path(&path).unwrap_or_default();
    sf.global.default_rules = paths;
    let text = serde_json::to_string_pretty(&sf).map_err(|e| e.to_string())?;
    save_settings_json(&text)
}

/// Load the three rule bundles referenced by `settings.json` for the form editor.
pub fn load_default_rules_bundles_form() -> Result<DefaultRulesBundlesForm, String> {
    let path = default_settings_path();
    let parent = path
        .parent()
        .ok_or_else(|| "invalid settings path".to_string())?;
    let sf = load_settings_from_path(&path).unwrap_or_default();
    let dr = &sf.global.default_rules;
    let block = read_rule_bundle_file(parent, dr.block_json.as_deref(), "block")?;
    let pass = read_rule_bundle_file(parent, dr.pass_json.as_deref(), "pass")?;
    let our = read_rule_bundle_file(parent, dr.our_json.as_deref(), "our")?;
    Ok(DefaultRulesBundlesForm { block, pass, our })
}

/// Write `user-*.json`, validate, and point `global.default_rules` at them.
pub fn save_default_rules_bundles_form(form: DefaultRulesBundlesForm) -> Result<(), String> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let block_json = serialize_rule_bundle_json(&form.block, "block", 1, &today)?;
    let pass_json = serialize_rule_bundle_json(&form.pass, "pass", 1, &today)?;
    let our_json = serialize_rule_bundle_json(&form.our, "our", 1, &today)?;
    validate_default_rules_json("block", &block_json)?;
    validate_default_rules_json("pass", &pass_json)?;
    validate_default_rules_json("our", &our_json)?;
    let dir = default_rules_target_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("user-block.json"), block_json).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("user-pass.json"), pass_json).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("user-our.json"), our_json).map_err(|e| e.to_string())?;
    let settings_path = default_settings_path();
    let mut sf = load_settings_from_path(&settings_path).unwrap_or_default();
    sf.global.default_rules = DefaultRulesPaths {
        block_json: Some(USER_BLOCK_REL.into()),
        pass_json: Some(USER_PASS_REL.into()),
        our_json: Some(USER_OUR_REL.into()),
    };
    let text = serde_json::to_string_pretty(&sf).map_err(|e| e.to_string())?;
    save_settings_json(&text)
}

/// Load [`BoardConfig`] for `default_board` and global bypass for the form.
pub fn load_board_rules_form() -> Result<BoardRulesForm, String> {
    let path = default_settings_path();
    let sf = load_settings_from_path(&path).unwrap_or_default();
    let board_id = if sf.default_board.trim().is_empty() {
        "default".to_string()
    } else {
        sf.default_board.clone()
    };
    let cfg = sf
        .boards
        .get(&board_id)
        .cloned()
        .unwrap_or_default();
    Ok(BoardRulesForm {
        traffic_rule_source: sf.global.traffic_rule_source.clone(),
        default_board: board_id.clone(),
        board_id,
        global_bypass: sf.global.bypass.clone(),
        bypass: cfg.bypass
    })
}

/// Merge board rules and global bypass into `settings.json`.
pub fn save_board_rules_form(f: BoardRulesForm) -> Result<(), String> {
    let bid = f.board_id.trim();
    if bid.is_empty() {
        return Err("board_id is empty".into());
    }
    let path = default_settings_path();
    let mut sf = load_settings_from_path(&path).unwrap_or_default();
    sf.default_board = f.default_board.trim().to_string();
    sf.global.traffic_rule_source = f.traffic_rule_source.trim().to_string();
    sf.global.bypass = f.global_bypass;
    let entry = sf
        .boards
        .entry(bid.to_string())
        .or_insert_with(BoardConfig::default);
    if TrafficRuleSource::parse(&f.traffic_rule_source) == TrafficRuleSource::Bundles {
        entry.bypass.clear();
    } else {
        entry.bypass = f.bypass;
    }
    let text = serde_json::to_string_pretty(&sf).map_err(|e| e.to_string())?;
    save_settings_json(&text)
}
