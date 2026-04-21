//! Named server URLs (several targets on the operator PC).

use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::sync::Once;

use crate::desktop_store;

static MIGRATE_PAIRING_FROM_CONNECTION: Once = Once::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerBookmark {
    pub id: String,
    pub label: String,
    pub url: String,
    /// Present when this bookmark was created from a paired install bundle (deploy auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_pubkey_b64: Option<String>,
    /// True when `server_pubkey_b64` is set and gRPC should send signed metadata for this URL.
    #[serde(default)]
    pub paired: bool,
    /// Out-of-band host-agent base URL (e.g. http://host:9443).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_agent_base_url: Option<String>,
    /// Bearer token for host-agent `/v1/*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_agent_token: Option<String>,
}

fn normalize_url(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

/// One-time copy of `connection` pairing into the matching bookmark row (upgrade path).
fn migrate_connection_pairing_into_bookmarks_once() {
    MIGRATE_PAIRING_FROM_CONNECTION.call_once(|| {
        let Ok(c) = desktop_store::open() else {
            return;
        };
        let row = c.query_row(
            "SELECT url, server_pubkey_b64, paired FROM connection WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            },
        );
        let Ok((url, pk_opt, paired)) = row else {
            return;
        };
        if !paired || url.trim().is_empty() {
            return;
        }
        let Some(pk) = pk_opt.filter(|s| !s.trim().is_empty()) else {
            return;
        };
        let _ = set_bookmark_pairing_inner(&url, pk);
    });
}

fn load_bookmarks_raw() -> Vec<ServerBookmark> {
    let Ok(c) = desktop_store::open() else {
        return Vec::new();
    };
    let mut stmt = match c.prepare(
        "SELECT id, label, url, server_pubkey_b64, paired, host_agent_base_url, host_agent_token FROM bookmarks ORDER BY rowid",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        Ok(ServerBookmark {
            id: row.get(0)?,
            label: row.get(1)?,
            url: row.get(2)?,
            server_pubkey_b64: row.get(3)?,
            paired: row.get::<_, i64>(4)? != 0,
            host_agent_base_url: row.get(5)?,
            host_agent_token: row.get(6)?,
        })
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

pub fn load_bookmarks() -> Vec<ServerBookmark> {
    migrate_connection_pairing_into_bookmarks_once();
    load_bookmarks_raw()
}

fn save_bookmarks(list: &[ServerBookmark]) -> Result<(), String> {
    let mut c = desktop_store::open().map_err(|e| e.to_string())?;
    let tx = c.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM bookmarks", [])
        .map_err(|e| e.to_string())?;
    for b in list {
        tx.execute(
            "INSERT INTO bookmarks (id, label, url, server_pubkey_b64, paired, host_agent_base_url, host_agent_token) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                b.id,
                b.label,
                b.url,
                b.server_pubkey_b64,
                if b.paired { 1 } else { 0 },
                b.host_agent_base_url,
                b.host_agent_token,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
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
    migrate_connection_pairing_into_bookmarks_once();
    let mut list = load_bookmarks_raw();
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
        server_pubkey_b64: None,
        paired: false,
        host_agent_base_url: None,
        host_agent_token: None,
    });
    save_bookmarks(&list)?;
    Ok(id)
}

fn set_bookmark_pairing_inner(url: &str, server_pubkey_b64: String) -> Result<(), String> {
    let url = normalize_url(url);
    if url.is_empty() {
        return Err("url is empty".into());
    }
    let mut list = load_bookmarks_raw();
    if let Some(b) = list.iter_mut().find(|b| normalize_url(&b.url) == url) {
        b.paired = true;
        b.server_pubkey_b64 = Some(server_pubkey_b64);
    } else {
        list.push(ServerBookmark {
            id: new_id(),
            label: url.clone(),
            url,
            paired: true,
            server_pubkey_b64: Some(server_pubkey_b64),
            host_agent_base_url: None,
            host_agent_token: None,
        });
    }
    save_bookmarks(&list)
}

/// Copy deploy pairing for a URL into the bookmark row (after successful `Pair` on that server).
pub fn set_bookmark_pairing(url: &str, server_pubkey_b64: String) -> Result<(), String> {
    set_bookmark_pairing_inner(url, server_pubkey_b64)
}

/// If this bookmark URL was paired before, returns server pubkey to restore the `connection` row.
pub fn bookmark_pairing_pubkey_for_url(url: &str) -> Option<String> {
    migrate_connection_pairing_into_bookmarks_once();
    let url = normalize_url(url);
    let list = load_bookmarks_raw();
    let b = list.iter().find(|b| normalize_url(&b.url) == url)?;
    if !b.paired {
        return None;
    }
    let pk = b.server_pubkey_b64.as_ref()?.trim();
    if pk.is_empty() {
        return None;
    }
    Some(pk.to_string())
}

pub fn remove_bookmark(id: &str) -> Result<(), String> {
    migrate_connection_pairing_into_bookmarks_once();
    let mut list = load_bookmarks_raw();
    list.retain(|b| b.id != id);
    save_bookmarks(&list)
}

/// Update display label for an existing bookmark (URL unchanged).
/// Save out-of-band host-agent URL and token for a bookmark (empty strings clear).
pub fn set_bookmark_host_agent(
    id: &str,
    host_agent_base_url: &str,
    host_agent_token: &str,
) -> Result<(), String> {
    migrate_connection_pairing_into_bookmarks_once();
    let mut list = load_bookmarks_raw();
    let b = list
        .iter_mut()
        .find(|b| b.id == id)
        .ok_or_else(|| "bookmark not found".to_string())?;
    let u = host_agent_base_url.trim();
    let t = host_agent_token.trim();
    b.host_agent_base_url = if u.is_empty() {
        None
    } else {
        Some(u.to_string())
    };
    b.host_agent_token = if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    };
    save_bookmarks(&list)
}

pub fn bookmark_by_id(id: &str) -> Option<ServerBookmark> {
    migrate_connection_pairing_into_bookmarks_once();
    load_bookmarks_raw().into_iter().find(|b| b.id == id)
}

/// Update display label for an existing bookmark (URL unchanged).
pub fn set_bookmark_label(id: &str, label: String) -> Result<(), String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("label is empty".into());
    }
    migrate_connection_pairing_into_bookmarks_once();
    let list = load_bookmarks_raw();
    let url = list
        .iter()
        .find(|b| b.id == id)
        .map(|b| b.url.as_str())
        .ok_or_else(|| "bookmark not found".to_string())?;
    upsert_bookmark(label, url)?;
    Ok(())
}
