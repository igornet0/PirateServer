//! Direct database access from the Tauri / desktop process (DBeaver-style).

mod adapter;
mod direct_password_store;
mod engines;
mod mysql_ops;
mod pg_ops;
mod profiles;
mod readonly;
mod redis_ops;
mod sessions;
mod stats;
mod structure;

pub use adapter::{DirectEngine, MysqlEngine, PgEngine, RedisEngine};
pub use engines::{
    clickhouse_not_implemented, is_direct_engine_implemented, mongo_not_implemented,
};
pub use engines::{ENGINE_CLICKHOUSE, ENGINE_MONGO, ENGINE_MYSQL, ENGINE_POSTGRES, ENGINE_REDIS};
pub use profiles::{
    direct_password_has, direct_password_set, direct_profile_delete, direct_profile_get_row,
    direct_profile_list_json, direct_profile_upsert, mark_profile_last_ok, query_history_append,
    query_history_list_json, DirectProfileRow, DirectProfileUpsert, DirectProfileView,
};

use serde::Serialize;
use std::time::Instant;

use crate::db_direct::pg_ops as pg;

const ERR_ENGINE: &str = "unsupported engine for this operation";

/// Connection parameters (password may be one-off; never log).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectConnectParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub ssl_mode: String,
    #[serde(default = "def_engine")]
    pub engine: String,
}

fn def_engine() -> String {
    ENGINE_POSTGRES.into()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResultView {
    pub columns: Vec<String>,
    pub row_count: usize,
    pub rows: Vec<serde_json::Value>,
    pub truncated: bool,
    pub warn: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectTestRequest {
    pub connect: DirectConnectParams,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectTestResponse {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub err: Option<String>,
}

/// Connection test (no session kept).
pub async fn direct_test(req: DirectTestRequest) -> Result<DirectTestResponse, String> {
    let e = req.connect.engine.to_ascii_lowercase();
    if e == ENGINE_POSTGRES {
        return match pg::pg_test_latency(&req.connect).await {
            Ok(ms) => Ok(DirectTestResponse {
                ok: true,
                latency_ms: Some(ms),
                err: None,
            }),
            Err(err) => Ok(DirectTestResponse {
                ok: false,
                latency_ms: None,
                err: Some(err),
            }),
        };
    }
    if e == ENGINE_MYSQL {
        return match mysql_ops::mysql_test_latency(&req.connect).await {
            Ok(ms) => Ok(DirectTestResponse {
                ok: true,
                latency_ms: Some(ms),
                err: None,
            }),
            Err(err) => Ok(DirectTestResponse {
                ok: false,
                latency_ms: None,
                err: Some(err),
            }),
        };
    }
    if e == ENGINE_REDIS {
        return match redis_ops::redis_test_latency(
            &req.connect.host,
            req.connect.port,
            &req.connect.password,
        )
        .await
        {
            Ok(ms) => Ok(DirectTestResponse {
                ok: true,
                latency_ms: Some(ms),
                err: None,
            }),
            Err(err) => Ok(DirectTestResponse {
                ok: false,
                latency_ms: None,
                err: Some(err),
            }),
        };
    }
    if e == ENGINE_CLICKHOUSE {
        return Err(clickhouse_not_implemented());
    }
    if e == ENGINE_MONGO {
        return Err(mongo_not_implemented());
    }
    Err(format!("{ERR_ENGINE}: {e}"))
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectOpenRequest {
    pub profile_id: String,
    /// Only used when no password is stored in the local JSON (prompt in UI).
    #[serde(default)]
    pub password: Option<String>,
}

fn resolve_password(profile_id: &str, password: Option<String>) -> Result<String, String> {
    if let Some(p) = password {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    profiles::direct_password_get(profile_id)?
        .ok_or_else(|| "missing password (enter password or save it when saving a profile; stored in db_direct_passwords.json)".into())
}

fn row_to_params(row: &DirectProfileRow, password: &str) -> DirectConnectParams {
    DirectConnectParams {
        host: row.host.clone(),
        port: row.port.clamp(1, 65535) as u16,
        username: row.username.clone().unwrap_or_else(|| "postgres".into()),
        password: password.to_string(),
        database: row.database_name.clone(),
        ssl_mode: if row.ssl_mode.is_empty() {
            "prefer".into()
        } else {
            row.ssl_mode.clone()
        },
        engine: if row.engine.is_empty() {
            ENGINE_POSTGRES.into()
        } else {
            row.engine.clone()
        },
    }
}

/// Opens a long-lived session (pool) keyed by `profile_id` (replaces an existing one).
pub async fn direct_open(req: DirectOpenRequest) -> Result<serde_json::Value, String> {
    let row = profiles::direct_profile_get_row(&req.profile_id)?
        .ok_or_else(|| "profile not found".to_string())?;
    let pass = resolve_password(&req.profile_id, req.password.clone())?;
    let p = row_to_params(&row, &pass);
    let eng = p.engine.to_ascii_lowercase();
    if eng == ENGINE_POSTGRES {
        let opts = pg::pg_options(&p);
        sessions::open_postgres_session(req.profile_id.clone(), opts).await?;
        let _ = mark_profile_last_ok(&req.profile_id);
        return Ok(serde_json::json!({ "ok": true, "sessionId": req.profile_id }));
    }
    if eng == ENGINE_MYSQL {
        let o = mysql_ops::mysql_options_from(&p)?;
        sessions::open_mysql_session(req.profile_id.clone(), o).await?;
        let _ = mark_profile_last_ok(&req.profile_id);
        return Ok(serde_json::json!({ "ok": true, "sessionId": req.profile_id }));
    }
    if eng == ENGINE_REDIS {
        let m = redis_ops::redis_open_manager(&p.host, p.port, &p.password)
            .await
            .map_err(|e| e.to_string())?;
        sessions::open_redis_session(req.profile_id.clone(), m).await?;
        let _ = mark_profile_last_ok(&req.profile_id);
        return Ok(serde_json::json!({ "ok": true, "sessionId": req.profile_id }));
    }
    if eng == ENGINE_CLICKHOUSE {
        return Err(clickhouse_not_implemented());
    }
    if eng == ENGINE_MONGO {
        return Err(mongo_not_implemented());
    }
    Err(format!("{ERR_ENGINE}: {eng}"))
}

pub fn direct_close(session_id: String) -> Result<serde_json::Value, String> {
    crate::tokio_runtime::block_on(sessions::session_close(&session_id))?;
    Ok(serde_json::json!({ "ok": true }))
}

/// List DBs / schemas / tables depending on engine.
pub async fn direct_list_databases(session_id: &str) -> Result<String, String> {
    if let Ok(pool) = sessions::pg_pool_for(session_id).await {
        let v = pg::pg_list_databases(&pool).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(pool) = sessions::mysql_pool_for(session_id).await {
        let v = mysql_ops::mysql_list_databases(&pool).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(mut con) = sessions::redis_manager_for(session_id).await {
        let _: String = redis::cmd("PING")
            .query_async(&mut con)
            .await
            .map_err(|e| e.to_string())?;
        return serde_json::to_string(&vec!["(default)"]).map_err(|e| e.to_string());
    }
    Err("session not found".into())
}

pub async fn direct_list_schemas(session_id: &str) -> Result<String, String> {
    if let Ok(pool) = sessions::pg_pool_for(session_id).await {
        let v = pg::pg_list_schemas(&pool).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(pool) = sessions::mysql_pool_for(session_id).await {
        let v = mysql_ops::mysql_list_schemas(&pool).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(_) = sessions::redis_manager_for(session_id).await {
        return Ok("[]".into());
    }
    Err("session not found".into())
}

pub async fn direct_list_tables(session_id: &str, schema: &str) -> Result<String, String> {
    if let Ok(pool) = sessions::pg_pool_for(session_id).await {
        let v = pg::pg_list_tables(&pool, schema).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(pool) = sessions::mysql_pool_for(session_id).await {
        let v = mysql_ops::mysql_list_tables(&pool, schema).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(mut con) = sessions::redis_manager_for(session_id).await {
        // Treat "tables" as a sample of key names (use SCAN; avoid KEYS in UI).
        let v = redis_ops::redis_sample_keys(&mut con, 500).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    Err("session not found (list tables)".into())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectPreviewRequest {
    pub session_id: String,
    pub schema: String,
    pub table: String,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

pub async fn direct_table_preview(req: DirectPreviewRequest) -> Result<String, String> {
    let limit = req.limit.unwrap_or(100);
    let offset = req.offset.unwrap_or(0);
    if let Ok(pool) = sessions::pg_pool_for(&req.session_id).await {
        let v = pg::pg_table_preview(&pool, &req.schema, &req.table, limit, offset).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    if let Ok(pool) = sessions::mysql_pool_for(&req.session_id).await {
        let v =
            mysql_ops::mysql_table_preview(&pool, &req.schema, &req.table, limit, offset).await?;
        return serde_json::to_string(&v).map_err(|e| e.to_string());
    }
    Err("session not found (preview: only PostgreSQL and MySQL)".into())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectQueryRequest {
    pub session_id: String,
    pub sql: String,
    pub max_rows: Option<u32>,
}

pub async fn direct_query(req: DirectQueryRequest) -> Result<String, String> {
    let max_rows = req.max_rows.unwrap_or(1000);
    let t0 = Instant::now();
    let res: Result<QueryResultView, String> = async {
        if let Ok(pool) = sessions::pg_pool_for(&req.session_id).await {
            return pg::pg_run_readonly_sql(&pool, &req.sql, max_rows).await;
        }
        if let Ok(pool) = sessions::mysql_pool_for(&req.session_id).await {
            return mysql_ops::mysql_run_readonly_sql(&pool, &req.sql, max_rows).await;
        }
        if let Ok(mut con) = sessions::redis_manager_for(&req.session_id).await {
            let sql = req.sql.trim();
            if sql.to_ascii_lowercase() == "ping" || sql.eq_ignore_ascii_case("PING") {
                let s: String = redis::cmd("PING")
                    .query_async(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(QueryResultView {
                    columns: vec!["PING".into()],
                    row_count: 1,
                    rows: vec![serde_json::json!({ "PING": s })],
                    truncated: false,
                    warn: None,
                });
            }
            if sql.to_ascii_lowercase().starts_with("info") {
                let section = sql.split_whitespace().nth(1).unwrap_or("default");
                let info: String = redis::cmd("INFO")
                    .arg(section)
                    .query_async(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(QueryResultView {
                    columns: vec!["line".into()],
                    row_count: 1,
                    rows: vec![serde_json::json!({ "line": info })],
                    truncated: true,
                    warn: Some("raw INFO; use a section name after INFO, e.g. INFO server".into()),
                });
            }
            if sql.to_ascii_lowercase().starts_with("keys ") {
                return Err(
                    "KEYS is not allowed; use KEY_SAMPLE or SCAN in this explorer build".into(),
                );
            }
            return Err(
                "For Redis, use: PING, INFO [section], or dedicated key browser (use tree)".into(),
            );
        }
        Err("session not found".into())
    }
    .await;
    let dur = t0.elapsed().as_millis() as u64;
    match res {
        Ok(v) => {
            let _ = query_history_append(&req.session_id, &req.sql, Some(dur), true, None);
            serde_json::to_string(&v).map_err(|e| e.to_string())
        }
        Err(e) => {
            let _ = query_history_append(&req.session_id, &req.sql, Some(dur), false, Some(&e));
            Err(e)
        }
    }
}

pub async fn direct_heartbeat(session_id: &str) -> Result<String, String> {
    if let Ok(p) = sessions::pg_pool_for(session_id).await {
        let ms = stats::pg_heartbeat_rtt_ms(&p).await;
        return Ok(serde_json::json!({ "ok": true, "rttMs": ms }).to_string());
    }
    if let Ok(p) = sessions::mysql_pool_for(session_id).await {
        let t0 = Instant::now();
        let _ = sqlx::query("SELECT 1")
            .fetch_one(&p)
            .await
            .map_err(|e| e.to_string())?;
        let ms = t0.elapsed().as_millis() as u64;
        return Ok(serde_json::json!({ "ok": true, "rttMs": ms }).to_string());
    }
    if let Ok(mut c) = sessions::redis_manager_for(session_id).await {
        let t0 = Instant::now();
        let _: String = redis::cmd("PING")
            .query_async(&mut c)
            .await
            .map_err(|e| e.to_string())?;
        let ms = t0.elapsed().as_millis() as u64;
        return Ok(serde_json::json!({ "ok": true, "rttMs": ms }).to_string());
    }
    Err("session not found".into())
}

pub async fn direct_pg_stats_json(session_id: &str) -> Result<String, String> {
    let p = sessions::pg_pool_for(session_id)
        .await
        .map_err(|e| e.to_string())?;
    let b = stats::pg_stats_bundle(&p).await;
    serde_json::to_string(&b).map_err(|e| e.to_string())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectStructureRequest {
    pub session_id: String,
    pub schema: String,
    pub table: String,
}

pub async fn direct_pg_structure_json(req: DirectStructureRequest) -> Result<String, String> {
    let p = sessions::pg_pool_for(&req.session_id)
        .await
        .map_err(|e| e.to_string())?;
    let s = structure::pg_table_structure(&p, &req.schema, &req.table).await?;
    serde_json::to_string(&s).map_err(|e| e.to_string())
}
