//! MySQL (sqlx) — read-only queries for Explorer.

use serde_json::json;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::mysql::MySqlRow;
use sqlx::Column;
use sqlx::MySqlPool;
use sqlx::Row;
use std::time::{Duration, Instant};

use super::readonly::is_readonly_sql;
use super::DirectConnectParams;
use super::QueryResultView;

fn mysql_opts(p: &DirectConnectParams) -> Result<MySqlConnectOptions, String> {
    let host = p.host.clone();
    let port = p.port;
    let user = p.username.clone();
    let pass = p.password.clone();
    let db = p
        .database
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let mut o = MySqlConnectOptions::new()
        .host(&host)
        .port(port)
        .username(&user)
        .password(&pass);
    if !db.is_empty() {
        o = o.database(db);
    }
    Ok(o)
}

pub async fn mysql_test_latency(p: &DirectConnectParams) -> Result<u64, String> {
    let opts = mysql_opts(p)?;
    let t0 = Instant::now();
    let pool = tokio::time::timeout(
        Duration::from_secs(8),
        super::sessions::mysql_pool_options().connect_with(opts),
    )
    .await
    .map_err(|_| "connection timeout (8s)".to_string())?
    .map_err(|e| e.to_string())?;
    sqlx::query("SELECT 1")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
    pool.close().await;
    Ok(t0.elapsed().as_millis() as u64)
}

fn mysql_row_to_json(row: &MySqlRow) -> serde_json::Value {
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
        } else if let Ok(x) = row.try_get::<String, _>(i) {
            json!(x)
        } else {
            json!(null)
        };
        map.insert(name, v);
    }
    serde_json::Value::Object(map)
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub async fn mysql_list_databases(pool: &MySqlPool) -> Result<Vec<String>, String> {
    let rows = sqlx::query("SHOW DATABASES")
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|r| r.try_get::<String, _>(0).map_err(|e| e.to_string()))
        .collect()
}

/// In MySQL, schema and database are largely synonymous; we list system `information_schema` names except noise.
pub async fn mysql_list_schemas(pool: &MySqlPool) -> Result<Vec<String>, String> {
    mysql_list_databases(pool).await
}

pub async fn mysql_list_tables(pool: &MySqlPool, schema: &str) -> Result<Vec<String>, String> {
    if !ident_ok(schema) {
        return Err("invalid schema name".into());
    }
    let q = format!(
        "SELECT table_name FROM information_schema.tables
         WHERE table_schema = '{schema}' AND table_type = 'BASE TABLE' ORDER BY table_name"
    );
    let rows = sqlx::query(&q)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|r| r.try_get::<String, _>(0).map_err(|e| e.to_string()))
        .collect()
}

pub async fn mysql_table_preview(
    pool: &MySqlPool,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
) -> Result<QueryResultView, String> {
    if !ident_ok(schema) || !ident_ok(table) {
        return Err("invalid schema or table name".into());
    }
    let lim = limit.max(1).min(5000);
    let off = offset.min(1_000_000_000);
    let sql = format!("SELECT * FROM `{schema}`.`{table}` LIMIT {lim} OFFSET {off}");
    is_readonly_sql(&sql).map_err(|e| e.to_string())?;
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let mut cols: Vec<String> = Vec::new();
    if let Some(r) = rows.first() {
        cols = r.columns().iter().map(|c| c.name().to_string()).collect();
    }
    let out: Vec<serde_json::Value> = rows.iter().map(mysql_row_to_json).collect();
    Ok(QueryResultView {
        columns: cols,
        row_count: out.len(),
        rows: out,
        truncated: false,
        warn: None,
    })
}

pub async fn mysql_run_readonly_sql(
    pool: &MySqlPool,
    sql: &str,
    max_rows: u32,
) -> Result<QueryResultView, String> {
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
        out.push(mysql_row_to_json(&r));
    }
    let truncated = total > cap;
    Ok(QueryResultView {
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

pub fn mysql_options_from(p: &DirectConnectParams) -> Result<MySqlConnectOptions, String> {
    mysql_opts(p)
}
