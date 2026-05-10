//! SQLite metadata for direct DB connection profiles; passwords in local JSON (encrypted, no OS keychain).

use crate::desktop_store::open as store_open;
use crate::desktop_store::db_path;
use crate::db_direct::direct_password_store;
use rusqlite::params;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Non-secret view for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectProfileView {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub host: String,
    pub port: i64,
    pub database_name: Option<String>,
    pub username: Option<String>,
    pub ssl_mode: String,
    pub group_tag: Option<String>,
    pub order_index: i64,
    pub last_ok_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub has_saved_password: bool,
}

/// Payload for create/update (password via separate key).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectProfileUpsert {
    pub id: Option<String>,
    pub name: String,
    pub engine: String,
    pub host: String,
    pub port: i64,
    pub database_name: Option<String>,
    pub username: Option<String>,
    pub ssl_mode: String,
    pub group_tag: Option<String>,
    pub order_index: Option<i64>,
}

pub fn direct_password_set(profile_id: &str, password: &str) -> Result<(), String> {
    direct_password_store::set(profile_id, password)
}

pub fn direct_password_get(profile_id: &str) -> Result<Option<String>, String> {
    direct_password_store::get(profile_id)
}

pub fn direct_password_has(profile_id: &str) -> Result<bool, String> {
    direct_password_store::has(profile_id)
}

pub fn direct_profile_list_json() -> Result<String, String> {
    let c = store_open().map_err(|e| e.to_string())?;
    let mut stmt = c
        .prepare(
            "SELECT id, name, engine, host, port, database_name, username, ssl_mode, group_tag,
                    order_index, last_ok_at_ms, created_at_ms, updated_at_ms
             FROM db_direct_profile ORDER BY order_index, name",
        )
        .map_err(|e| e.to_string())?;
    let mut out: Vec<DirectProfileView> = Vec::new();
    let rows = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            Ok(DirectProfileView {
                id: id.clone(),
                name: r.get(1)?,
                engine: r.get(2)?,
                host: r.get(3)?,
                port: r.get(4)?,
                database_name: r.get(5)?,
                username: r.get(6)?,
                ssl_mode: r.get(7)?,
                group_tag: r.get(8)?,
                order_index: r.get(9)?,
                last_ok_at_ms: r.get(10)?,
                created_at_ms: r.get(11)?,
                updated_at_ms: r.get(12)?,
                has_saved_password: false,
            })
        })
        .map_err(|e| e.to_string())?;
    for row in rows.filter_map(|x| x.ok()) {
        let mut v = row;
        v.has_saved_password = direct_password_has(&v.id).unwrap_or(false);
        out.push(v);
    }
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

pub fn direct_profile_upsert(
    body: &DirectProfileUpsert,
    password: Option<&str>,
) -> Result<String, String> {
    let id = body
        .id
        .as_ref()
        .map(|s| s.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let t = now_ms();
    let c = store_open().map_err(|e| e.to_string())?;
    c.execute(
        "INSERT INTO db_direct_profile
            (id, name, engine, host, port, database_name, username, ssl_mode, group_tag,
             order_index, last_ok_at_ms, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            engine = excluded.engine,
            host = excluded.host,
            port = excluded.port,
            database_name = excluded.database_name,
            username = excluded.username,
            ssl_mode = excluded.ssl_mode,
            group_tag = excluded.group_tag,
            order_index = excluded.order_index,
            updated_at_ms = excluded.updated_at_ms",
        params![
            &id,
            &body.name,
            &body.engine,
            &body.host,
            &body.port,
            &body.database_name,
            &body.username,
            &body.ssl_mode,
            &body.group_tag,
            &body.order_index.unwrap_or(0i64),
            t,
            t,
        ],
    )
    .map_err(|e| e.to_string())?;
    if let Some(p) = password {
        if !p.is_empty() {
            direct_password_set(&id, p)?;
        }
    }
    Ok(id)
}

/// Load a single profile row (no password).
pub fn direct_profile_get_row(id: &str) -> Result<Option<DirectProfileRow>, String> {
    let c = store_open().map_err(|e| e.to_string())?;
    let mut stmt = c
        .prepare(
            "SELECT id, name, engine, host, port, database_name, username, ssl_mode, group_tag,
                    order_index, last_ok_at_ms, created_at_ms, updated_at_ms
             FROM db_direct_profile WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let row = match stmt.query_row([id], |r| {
        Ok(DirectProfileRow {
            id: r.get(0)?,
            name: r.get(1)?,
            engine: r.get(2)?,
            host: r.get(3)?,
            port: r.get(4)?,
            database_name: r.get(5)?,
            username: r.get(6)?,
            ssl_mode: r.get(7)?,
            group_tag: r.get(8)?,
            order_index: r.get(9)?,
            last_ok_at_ms: r.get(10)?,
            created_at_ms: r.get(11)?,
            updated_at_ms: r.get(12)?,
        })
    }) {
        Ok(x) => Some(x),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e.to_string()),
    };
    Ok(row)
}

#[derive(Debug, Clone)]
pub struct DirectProfileRow {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub host: String,
    pub port: i64,
    pub database_name: Option<String>,
    pub username: Option<String>,
    pub ssl_mode: String,
    #[allow(dead_code)]
    pub group_tag: Option<String>,
    #[allow(dead_code)]
    pub order_index: i64,
    #[allow(dead_code)]
    pub last_ok_at_ms: Option<i64>,
    #[allow(dead_code)]
    pub created_at_ms: i64,
    #[allow(dead_code)]
    pub updated_at_ms: i64,
}

pub fn direct_profile_delete(id: &str) -> Result<(), String> {
    let c = store_open().map_err(|e| e.to_string())?;
    c.execute("DELETE FROM db_direct_query_history WHERE connection_id = ?1", [id])
        .map_err(|e| e.to_string())?;
    c.execute("DELETE FROM db_direct_profile WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    let _ = direct_password_set(id, "");
    Ok(())
}

pub fn mark_profile_last_ok(id: &str) -> Result<(), String> {
    let c = store_open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE db_direct_profile SET last_ok_at_ms = ?1, updated_at_ms = ?1 WHERE id = ?2",
        params![now_ms(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Append a query history row (no full SQL in logs).
pub fn query_history_append(
    connection_id: &str,
    sql_snippet: &str,
    duration_ms: Option<u64>,
    ok: bool,
    err: Option<&str>,
) -> Result<(), String> {
    let c = store_open().map_err(|e| e.to_string())?;
    let cap = sql_snippet.chars().count().min(4000);
    let snippet: String = sql_snippet.chars().take(cap).collect();
    c.execute(
        "INSERT INTO db_direct_query_history (connection_id, sql_text, ts_ms, duration_ms, ok, err)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            connection_id,
            &snippet,
            now_ms(),
            duration_ms.map(|d| d as i64),
            if ok { 1i64 } else { 0i64 },
            err,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn query_history_list_json(connection_id: &str, limit: i64) -> Result<String, String> {
    let c = store_open().map_err(|e| e.to_string())?;
    let lim = limit.clamp(1, 200);
    let mut stmt = c
        .prepare(
            "SELECT id, connection_id, sql_text, ts_ms, duration_ms, ok, err
             FROM db_direct_query_history
             WHERE connection_id = ?1
             ORDER BY ts_ms DESC
             LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<serde_json::Value> = stmt
        .query_map(params![connection_id, lim], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "connectionId": r.get::<_, String>(1)?,
                "sqlText": r.get::<_, String>(2)?,
                "tsMs": r.get::<_, i64>(3)?,
                "durationMs": r.get::<_, Option<i64>>(4)?,
                "ok": r.get::<_, i64>(5)? != 0,
                "err": r.get::<_, Option<String>>(6)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    serde_json::to_string(&rows).map_err(|e| e.to_string())
}

/// Debug path (for support); avoid exposing in production UI.
#[allow(dead_code)]
pub fn _store_path() -> String {
    db_path().to_string_lossy().to_string()
}
