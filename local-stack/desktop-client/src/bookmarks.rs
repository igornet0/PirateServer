//! Named server URLs (variant A: several targets on the operator PC).

use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerBookmark {
    pub id: String,
    pub label: String,
    pub url: String,
}

fn bookmarks_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("server_bookmarks.json")
}

fn normalize_url(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

pub fn load_bookmarks() -> Vec<ServerBookmark> {
    let path = bookmarks_path();
    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_bookmarks(list: &[ServerBookmark]) -> Result<(), String> {
    let path = bookmarks_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(list).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn new_id() -> String {
    format!("{:016x}", OsRng.next_u64())
}

/// Insert or update bookmark by URL; returns bookmark id.
pub fn upsert_bookmark(label: impl Into<String>, url: &str) -> Result<String, String> {
    let url = normalize_url(url);
    if url.is_empty() {
        return Err("url is empty".into());
    }
    let mut list = load_bookmarks();
    if let Some(b) = list.iter_mut().find(|b| normalize_url(&b.url) == url) {
        b.label = label.into();
        let id = b.id.clone();
        save_bookmarks(&list)?;
        return Ok(id);
    }
    let id = new_id();
    list.push(ServerBookmark {
        id: id.clone(),
        label: label.into(),
        url,
    });
    save_bookmarks(&list)?;
    Ok(id)
}

pub fn remove_bookmark(id: &str) -> Result<(), String> {
    let mut list = load_bookmarks();
    list.retain(|b| b.id != id);
    save_bookmarks(&list)
}
