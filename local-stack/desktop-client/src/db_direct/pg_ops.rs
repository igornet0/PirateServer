//! PostgreSQL helpers (sqlx).

use serde_json::json;
use sqlx::postgres::PgConnectOptions;
use sqlx::postgres::PgRow;
use sqlx::Column;
use sqlx::PgPool;
use sqlx::Row;
use std::time::{Duration, Instant};

use super::readonly::is_readonly_sql;
use super::DirectConnectParams;

fn ssl_mode_from(s: &str) -> sqlx::postgres::PgSslMode {
    match s.to_ascii_lowercase().as_str() {
        "disable" => sqlx::postgres::PgSslMode::Disable,
        "allow" => sqlx::postgres::PgSslMode::Allow,
        "prefer" => sqlx::postgres::PgSslMode::Prefer,
        "require" | "on" | "true" | "1" => sqlx::postgres::PgSslMode::Require,
        "verify-full" | "verify_full" => sqlx::postgres::PgSslMode::Require,
        _ => sqlx::postgres::PgSslMode::Prefer,
    }
}

/// Build connect options; `database` may be `postgres` for listing DBs.
pub fn pg_options(p: &DirectConnectParams) -> PgConnectOptions {
    let db = p
        .database
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("postgres");
    PgConnectOptions::new()
        .host(&p.host)
        .port(p.port)
        .username(&p.username)
        .password(&p.password)
        .database(db)
        .ssl_mode(ssl_mode_from(&p.ssl_mode))
}

pub async fn pg_test_latency(p: &DirectConnectParams) -> Result<u64, String> {
    let opts = pg_options(p);
    let t0 = Instant::now();
    let pool = tokio::time::timeout(Duration::from_secs(8), PgPool::connect_with(opts))
        .await
        .map_err(|_| "connection timeout (8s)".to_string())?
        .map_err(|e| e.to_string())?;
    let _ = sqlx::query("SELECT 1")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
    pool.close().await;
    Ok(t0.elapsed().as_millis() as u64)
}

fn pg_row_to_json(row: &PgRow) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name().to_string();
        let v: serde_json::Value = if let Ok(x) = row.try_get::<Option<String>, _>(i) {
            json!(x)
        } else if let Ok(x) = row.try_get::<Option<i64>, _>(i) {
            json!(x)
        } else if let Ok(x) = row.try_get::<Option<i32>, _>(i) {
            json!(x)
        } else if let Ok(x) = row.try_get::<Option<f64>, _>(i) {
            json!(x)
        } else if let Ok(x) = row.try_get::<Option<bool>, _>(i) {
            json!(x)
        } else if let Ok(x) = row.try_get::<String, _>(i) {
            json!(x)
        } else if let Ok(x) = row.try_get::<Vec<u8>, _>(i) {
            json!(format!("<binary {} bytes>", x.len()))
        } else {
            json!(null)
        };
        map.insert(name, v);
    }
    serde_json::Value::Object(map)
}

pub async fn pg_list_databases(pool: &PgPool) -> Result<Vec<String>, String> {
    let rows =
        sqlx::query("SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|r| r.try_get::<String, _>(0).map_err(|e| e.to_string()))
        .collect()
}

pub async fn pg_list_schemas(pool: &PgPool) -> Result<Vec<String>, String> {
    let rows = sqlx::query(
        "SELECT nspname::text AS n
         FROM pg_catalog.pg_namespace
         WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
           AND nspname NOT LIKE 'pg\\_%' ESCAPE '\\'
         ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|r| r.try_get::<String, _>(0).map_err(|e| e.to_string()))
        .collect()
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub async fn pg_list_tables(pool: &PgPool, schema: &str) -> Result<Vec<String>, String> {
    if !ident_ok(schema) {
        return Err("invalid schema name".into());
    }
    let q = format!(
        "SELECT table_name FROM information_schema.tables
         WHERE table_schema = '{schema}' AND table_type = 'BASE TABLE'
         ORDER BY table_name"
    );
    let rows = sqlx::query(&q)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|r| r.try_get::<String, _>(0).map_err(|e| e.to_string()))
        .collect()
}

pub async fn pg_table_preview(
    pool: &PgPool,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
) -> Result<super::QueryResultView, String> {
    if !ident_ok(schema) || !ident_ok(table) {
        return Err("invalid schema or table name".into());
    }
    let lim = limit.max(1).min(5000);
    let off = offset.min(1_000_000_000);
    let sql = format!("SELECT * FROM \"{schema}\".\"{table}\" LIMIT {lim} OFFSET {off}");
    is_readonly_sql(&sql).map_err(|e| e.to_string())?;
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let mut cols: Vec<String> = Vec::new();
    if let Some(r) = rows.first() {
        cols = r.columns().iter().map(|c| c.name().to_string()).collect();
    }
    let out: Vec<serde_json::Value> = rows.iter().map(pg_row_to_json).collect();
    Ok(super::QueryResultView {
        columns: cols,
        row_count: out.len(),
        rows: out,
        truncated: false,
        warn: None,
    })
}

pub async fn pg_run_readonly_sql(
    pool: &PgPool,
    sql: &str,
    max_rows: u32,
) -> Result<super::QueryResultView, String> {
    is_readonly_sql(sql).map_err(|e| e.to_string())?;
    let cap = (max_rows as usize).min(5000);
    let q = sql.trim();
    let rows = sqlx::query(q)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let total = rows.len();
    let n = total.min(cap);
    let mut cols: Vec<String> = Vec::new();
    if let Some(r) = rows.first() {
        cols = r.columns().iter().map(|c| c.name().to_string()).collect();
    }
    let mut out = Vec::new();
    for r in rows.into_iter().take(n) {
        out.push(pg_row_to_json(&r));
    }
    let truncated = total > cap;
    Ok(super::QueryResultView {
        columns: cols,
        row_count: out.len(),
        rows: out,
        truncated,
        warn: if truncated {
            Some("result truncated to max_rows".into())
        } else {
            None
        },
    })
}
