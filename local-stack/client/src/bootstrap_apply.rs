//! Merge `pirate-bootstrap` / `pirate-proxy-session` JSON into `settings.json` (per-board gRPC URL + session token).

use crate::settings::{load_settings_from_path, SettingsFile};
use serde_json::Value;
use std::path::Path;

pub fn apply_bootstrap_json(
    json: &Value,
    session_token_override: Option<&str>,
    settings_path: &Path,
) -> Result<(), String> {
    let typ = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if typ != "pirate-bootstrap" && typ != "pirate-proxy-session" {
        return Err(format!(
            "unsupported type {typ:?}: expected pirate-bootstrap or pirate-proxy-session"
        ));
    }
    let board_label = json
        .get("board_label")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing board_label".to_string())?
        .to_string();
    let grpc_url = json
        .get("grpc_url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let session_token = session_token_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            json.get("session_token")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| {
            "missing session_token: pass --session-token or include non-null session_token in JSON"
                .to_string()
        })?;
    let mut file: SettingsFile = load_settings_from_path(settings_path)
        .map_err(|e| format!("load settings: {e}"))?;
    let board = file.boards.entry(board_label.clone()).or_default();
    board.enabled = true;
    if let Some(u) = grpc_url {
        board.url = Some(u);
    }
    board.session_token = Some(session_token);
    if file.default_board.trim().is_empty() {
        file.default_board = board_label;
    }
    std::fs::write(
        settings_path,
        serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write settings: {e}"))?;
    Ok(())
}

pub async fn load_json_from_file_or_url(
    file: Option<&Path>,
    url: Option<&str>,
) -> Result<Value, String> {
    if let Some(p) = file {
        let text = std::fs::read_to_string(p).map_err(|e| format!("read file: {e}"))?;
        return serde_json::from_str(&text).map_err(|e| format!("parse JSON: {e}"));
    }
    if let Some(u) = url {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?;
        let r = client.get(u).send().await.map_err(|e| e.to_string())?;
        let status = r.status();
        let text = r.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {status}: {text}"));
        }
        return serde_json::from_str(&text).map_err(|e| format!("parse JSON: {e}"));
    }
    Err("provide --file PATH or --url URL".to_string())
}
