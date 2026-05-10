//! Host-side database discovery and multi-engine read-only access.
//! Instance id: `{engine}|{host}|{port}` (e.g. `postgresql|127.0.0.1|5432`).

use crate::types::{
    HostDatabaseCapabilities, HostDatabaseInstanceView, HostDatabaseQueryResultView,
    HostDatabaseRedisKeyView, HostDatabaseRedisKeysView, HostDatabasesListView,
    DatabaseColumnsView, DatabaseRelationshipsView, DatabaseSchemasView, DatabaseTablePreviewView,
    DatabaseTablesView,
};
use deploy_db::{
    explorer_columns, explorer_foreign_keys, explorer_schemas, explorer_table_preview, explorer_tables,
    validate_pg_ident, PgPool,
};
use serde_json::json;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::postgres::PgPoolOptions;
use sqlx::Column;
use sqlx::Row;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::net::TcpStream;
use url::Url;

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
const QUERY_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_SCAN_KEYS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum DbHostError {
    #[error("invalid instance id")]
    InvalidInstanceId,
    #[error("engine not supported: {0}")]
    UnsupportedEngine(String),
    #[error("connection or query failed: {0}")]
    Backend(String),
    #[error("SQL not allowed (read-only): {0}")]
    SqlPolicy(String),
    #[error("capability not available for this engine")]
    Capability,
    #[error("not found")]
    NotFound,
}

pub(crate) fn read_host_env_file(path: &Path) -> HashMap<String, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    let mut m = HashMap::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if !key.is_empty() {
            m.insert(key.to_string(), v.trim().to_string());
        }
    }
    m
}

pub(crate) fn env_get(m: &HashMap<String, String>, key: &str) -> Option<String> {
    m.get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `PIRATE_MYSQL_EXPLORER_URL` from the host env file (no per-request creds).
pub fn mysql_explorer_url_from_env(env_path: &Path) -> Result<String, DbHostError> {
    let m = read_host_env_file(env_path);
    env_get(&m, "PIRATE_MYSQL_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MYSQL_EXPLORER_URL not set".into()))
}

pub fn make_instance_id(engine: &str, host: &str, port: u16) -> String {
    format!("{engine}|{host}|{port}")
}

/// Parse `engine|host|port`.
pub fn parse_instance_id(id: &str) -> Option<(String, String, u16)> {
    let parts: Vec<&str> = id.split('|').collect();
    if parts.len() < 3 {
        return None;
    }
    let n = parts.len();
    let port: u16 = parts[n - 1].parse().ok()?;
    let host = parts[n - 2].to_string();
    let engine = parts[0..n - 2].join("|");
    if engine.is_empty() {
        return None;
    }
    Some((engine, host, port))
}

/// Short-lived pool for ad-hoc credentials (e.g. per-request headers).
pub async fn connect_ephemeral_postgres(url: &str) -> Result<PgPool, DbHostError> {
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))
}

/// Build a postgres:// URL for `instance_id` using username/password; path is the database.
pub fn postgres_url_with_creds(
    instance_id: &str,
    user: &str,
    pass: &str,
    database: &str,
) -> Result<String, DbHostError> {
    let (engine, host, port) = parse_instance_id(instance_id).ok_or(DbHostError::InvalidInstanceId)?;
    if engine != "postgresql" {
        return Err(DbHostError::UnsupportedEngine(engine));
    }
    let mut u = Url::parse(&format!("postgres://{host}:{port}/"))
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    u.set_username(user)
        .map_err(|_| DbHostError::Backend("URL: invalid username".into()))?;
    u.set_password(Some(pass))
        .map_err(|_| DbHostError::Backend("URL: invalid password".into()))?;
    u.set_path(&format!("/{database}"));
    Ok(u.to_string())
}

pub fn mysql_url_with_creds(
    instance_id: &str,
    user: &str,
    pass: &str,
) -> Result<String, DbHostError> {
    let (engine, host, port) = parse_instance_id(instance_id).ok_or(DbHostError::InvalidInstanceId)?;
    if engine != "mysql" {
        return Err(DbHostError::UnsupportedEngine(engine));
    }
    let mut u = Url::parse(&format!("mysql://{host}:{port}/"))
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    u.set_username(user)
        .map_err(|_| DbHostError::Backend("URL: invalid username".into()))?;
    u.set_password(Some(pass))
        .map_err(|_| DbHostError::Backend("URL: invalid password".into()))?;
    Ok(u.to_string())
}

/// `http://host:port` or `http://user:pass@host:port` (path preserved if any).
pub fn clickhouse_base_url_with_creds(
    instance_id: &str,
    user: &str,
    pass: &str,
) -> Result<String, DbHostError> {
    let (engine, host, port) = parse_instance_id(instance_id).ok_or(DbHostError::InvalidInstanceId)?;
    if engine != "clickhouse" {
        return Err(DbHostError::UnsupportedEngine(engine));
    }
    let mut u = Url::parse(&format!("http://{host}:{port}/"))
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    u.set_username(user)
        .map_err(|_| DbHostError::Backend("URL: invalid username".into()))?;
    u.set_password(Some(pass))
        .map_err(|_| DbHostError::Backend("URL: invalid password".into()))?;
    let s = u.to_string();
    Ok(s.trim_end_matches('/').to_string())
}

pub fn redis_url_with_creds(
    instance_id: &str,
    user: &str,
    pass: &str,
) -> Result<String, DbHostError> {
    let (engine, host, port) = parse_instance_id(instance_id).ok_or(DbHostError::InvalidInstanceId)?;
    if engine != "redis" {
        return Err(DbHostError::UnsupportedEngine(engine));
    }
    let mut u = Url::parse(&format!("redis://{host}:{port}/0"))
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    if !user.is_empty() {
        u.set_username(user)
            .map_err(|_| DbHostError::Backend("URL: invalid username".into()))?;
    }
    if !pass.is_empty() {
        u.set_password(Some(pass))
            .map_err(|_| DbHostError::Backend("URL: invalid password".into()))?;
    }
    Ok(u.to_string())
}

pub fn mongodb_url_with_creds(
    instance_id: &str,
    user: &str,
    pass: &str,
) -> Result<String, DbHostError> {
    let (engine, host, port) = parse_instance_id(instance_id).ok_or(DbHostError::InvalidInstanceId)?;
    if engine != "mongodb" {
        return Err(DbHostError::UnsupportedEngine(engine));
    }
    let mut u = Url::parse(&format!("mongodb://{host}:{port}/"))
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    u.set_username(user)
        .map_err(|_| DbHostError::Backend("URL: invalid username".into()))?;
    u.set_password(Some(pass))
        .map_err(|_| DbHostError::Backend("URL: invalid password".into()))?;
    Ok(u.to_string())
}

async fn tcp_probe(host: &str, port: u16) -> bool {
    let addr = format!("{host}:{port}");
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => true,
        _ => false,
    }
}

fn default_host(m: &HashMap<String, String>) -> String {
    env_get(m, "PIRATE_DB_BIND_HOST").unwrap_or_else(|| "127.0.0.1".to_string())
}

fn caps_none() -> HostDatabaseCapabilities {
    HostDatabaseCapabilities {
        metadata: false,
        list_schemas: false,
        list_tables: false,
        list_columns: false,
        preview_rows: false,
        foreign_keys: false,
        run_readonly_sql: false,
        list_redis_keys: false,
        list_mongo_databases: false,
        list_mongo_collections: false,
        preview_mongo_documents: false,
        clickhouse_system: false,
    }
}

fn caps_pg() -> HostDatabaseCapabilities {
    HostDatabaseCapabilities {
        metadata: true,
        list_schemas: true,
        list_tables: true,
        list_columns: true,
        preview_rows: true,
        foreign_keys: true,
        run_readonly_sql: true,
        list_redis_keys: false,
        list_mongo_databases: false,
        list_mongo_collections: false,
        preview_mongo_documents: false,
        clickhouse_system: false,
    }
}

fn caps_mysql() -> HostDatabaseCapabilities {
    caps_pg()
}

fn caps_redis() -> HostDatabaseCapabilities {
    let mut c = caps_none();
    c.metadata = true;
    c.list_redis_keys = true;
    c
}

fn caps_mongo() -> HostDatabaseCapabilities {
    let mut c = caps_none();
    c.metadata = true;
    c.list_mongo_databases = true;
    c.list_mongo_collections = true;
    c.preview_mongo_documents = true;
    c
}

fn caps_ch() -> HostDatabaseCapabilities {
    let mut c = caps_none();
    c.metadata = true;
    c.list_schemas = true;
    c.list_tables = true;
    c.list_columns = true;
    c.preview_rows = true;
    c.run_readonly_sql = true;
    c.clickhouse_system = true;
    c
}

fn caps_oracle() -> HostDatabaseCapabilities {
    caps_none()
}

fn pg_matches_explorer(
    pg_explorer_target: Option<&(String, u16)>,
    host: &str,
    port: u16,
) -> bool {
    let Some((h, p)) = pg_explorer_target else {
        return false;
    };
    h == host && *p == port
}

pub async fn list_instances(
    env_path: &Path,
    pg_explorer_target: Option<&(String, u16)>,
) -> HostDatabasesListView {
    let m = read_host_env_file(env_path);
    let host = default_host(&m);

    let pg_port: u16 = env_get(&m, "PIRATE_POSTGRESQL_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5432);
    let mysql_port: u16 = env_get(&m, "PIRATE_MYSQL_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3306);
    let redis_port: u16 = env_get(&m, "PIRATE_REDIS_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(6379);
    let mongo_port: u16 = env_get(&m, "PIRATE_MONGO_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(27017);
    let ch_http: u16 = env_get(&m, "PIRATE_CLICKHOUSE_HTTP_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8123);
    let oracle_port: u16 = env_get(&m, "PIRATE_ORACLE_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1521);

    let mut instances = Vec::new();

    if tcp_probe(&host, pg_port).await {
        let full = pg_matches_explorer(pg_explorer_target, &host, pg_port);
        // Per-request `X-Pirate-Db-*` creds can browse without POSTGRES_EXPLORER_URL; list full
        // capabilities whenever the port is up so the desktop UI is not empty.
        let c = caps_pg();
        let note = if !full {
            Some(
                "Optional: set POSTGRES_EXPLORER_URL in /etc/pirate-deploy.env to match this host:port and restart control-api for server-side pool + dashboard explorer; browsing with per-request credentials in the app works without it."
                    .to_string(),
            )
        } else {
            None
        };
        instances.push(HostDatabaseInstanceView {
            id: make_instance_id("postgresql", &host, pg_port),
            engine: "postgresql".into(),
            label: "PostgreSQL".into(),
            host: host.clone(),
            port: pg_port,
            reachable: true,
            dsn_template: format!("postgresql://{{user}}:{{password}}@{host}:{pg_port}/{{database}}"),
            connection_note: note,
            capabilities: c,
        });
    }

    if tcp_probe(&host, mysql_port).await {
        let has = env_get(&m, "PIRATE_MYSQL_EXPLORER_URL").is_some();
        let c = caps_mysql();
        let note = if has {
            None
        } else {
            Some(
                "Optional: set PIRATE_MYSQL_EXPLORER_URL on the host for env-based access; the desktop app can use per-request DB credentials without it."
                    .to_string(),
            )
        };
        instances.push(HostDatabaseInstanceView {
            id: make_instance_id("mysql", &host, mysql_port),
            engine: "mysql".into(),
            label: "MySQL".into(),
            host: host.clone(),
            port: mysql_port,
            reachable: true,
            dsn_template: format!("mysql://{{user}}:{{password}}@{host}:{mysql_port}/{{database}}"),
            connection_note: note,
            capabilities: c,
        });
    }

    if tcp_probe(&host, redis_port).await {
        let has = env_get(&m, "PIRATE_REDIS_URL").is_some();
        let c = caps_redis();
        let note = if has {
            None
        } else {
            Some(
                "Optional: set PIRATE_REDIS_URL on the host; key scan in the app can use per-request creds to redis://user:pass@host:port."
                    .to_string(),
            )
        };
        instances.push(HostDatabaseInstanceView {
            id: make_instance_id("redis", &host, redis_port),
            engine: "redis".into(),
            label: "Redis".into(),
            host: host.clone(),
            port: redis_port,
            reachable: true,
            dsn_template: format!("redis://:{{password}}@{host}:{redis_port}/{{db}}"),
            connection_note: note,
            capabilities: c,
        });
    }

    if tcp_probe(&host, mongo_port).await {
        let has = env_get(&m, "PIRATE_MONGO_EXPLORER_URL").is_some();
        let c = caps_mongo();
        let note = if has {
            None
        } else {
            Some(
                "Optional: set PIRATE_MONGO_EXPLORER_URL on the host; browsing in the app can use per-request creds and mongosh on the control-api host."
                    .to_string(),
            )
        };
        instances.push(HostDatabaseInstanceView {
            id: make_instance_id("mongodb", &host, mongo_port),
            engine: "mongodb".into(),
            label: "MongoDB".into(),
            host: host.clone(),
            port: mongo_port,
            reachable: true,
            dsn_template: format!("mongodb://{{user}}:{{password}}@{host}:{mongo_port}/{{database}}"),
            connection_note: note,
            capabilities: c,
        });
    }

    if tcp_probe(&host, ch_http).await {
        let has = env_get(&m, "PIRATE_CLICKHOUSE_HTTP_URL").is_some();
        let c = caps_ch();
        let note = if has {
            None
        } else {
            Some(
                "Optional: set PIRATE_CLICKHOUSE_HTTP_URL on the host; the app can pass DB user/password per request for clickhouse|host|port."
                    .to_string(),
            )
        };
        instances.push(HostDatabaseInstanceView {
            id: make_instance_id("clickhouse", &host, ch_http),
            engine: "clickhouse".into(),
            label: "ClickHouse".into(),
            host: host.clone(),
            port: ch_http,
            reachable: true,
            dsn_template: format!("http://{{user}}:{{password}}@{host}:{ch_http}/{{database}}"),
            connection_note: note,
            capabilities: c,
        });
    }

    if tcp_probe(&host, oracle_port).await {
        instances.push(HostDatabaseInstanceView {
            id: make_instance_id("oracle", &host, oracle_port),
            engine: "oracle".into(),
            label: "Oracle".into(),
            host: host.clone(),
            port: oracle_port,
            reachable: true,
            dsn_template: format!("oracle://{{user}}:{{password}}@{host}:{oracle_port}/{{service_name}}"),
            connection_note: Some("Use DBeaver/JDBC; no server-side driver.".into()),
            capabilities: caps_oracle(),
        });
    }

    HostDatabasesListView { instances }
}

pub fn is_readonly_sql(sql: &str) -> Result<(), DbHostError> {
    let t = sql.trim();
    if t.is_empty() {
        return Err(DbHostError::SqlPolicy("empty SQL".into()));
    }
    if t.contains(';') {
        let ttrim = t.trim_end();
        if ttrim.contains(';') {
            let without_trailing = ttrim.trim_end_matches(';');
            if without_trailing.contains(';') {
                return Err(DbHostError::SqlPolicy(
                    "multiple statements are not allowed".into(),
                ));
            }
        }
    }
    let up = t.to_uppercase();
    let first = up.split_whitespace().next().unwrap_or("");
    if matches!(
        first,
        "SELECT" | "SHOW" | "EXPLAIN" | "DESCRIBE" | "DESC" | "WITH" | "HELP" | "TABLES"
    ) {
        return Ok(());
    }
    if matches!(
        first,
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "ALTER" | "DROP" | "CREATE"
            | "TRUNCATE" | "GRANT" | "REVOKE" | "CALL" | "DO" | "SET" | "USE" | "START"
            | "BEGIN" | "COMMIT" | "ROLLBACK" | "LOCK" | "UNLOCK" | "LOAD" | "COPY"
    ) {
        return Err(DbHostError::SqlPolicy(format!(
            "write / DDL / session control not allowed: {first}"
        )));
    }
    Err(DbHostError::SqlPolicy(format!(
        "only read-only SQL is allowed (got: {first})"
    )))
}

fn pg_row_to_json(row: &sqlx::postgres::PgRow) -> serde_json::Value {
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
        } else {
            json!(null)
        };
        map.insert(name, v);
    }
    serde_json::Value::Object(map)
}

fn mysql_row_to_json(row: &sqlx::mysql::MySqlRow) -> serde_json::Value {
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
        } else {
            json!(null)
        };
        map.insert(name, v);
    }
    serde_json::Value::Object(map)
}

pub async fn pg_query(
    pool: &PgPool,
    sql: &str,
    max_rows: u32,
) -> Result<HostDatabaseQueryResultView, DbHostError> {
    is_readonly_sql(sql)?;
    let cap = (max_rows as usize).min(5_000);
    let q = sql.trim();
    let rows = sqlx::query(q)
        .fetch_all(pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
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
    Ok(HostDatabaseQueryResultView {
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

pub async fn mysql_query(
    env_path: &Path,
    instance_id: &str,
    sql: &str,
    max_rows: u32,
) -> Result<HostDatabaseQueryResultView, DbHostError> {
    let (engine, _host, _port) = parse_instance_id(instance_id).ok_or(DbHostError::InvalidInstanceId)?;
    if engine != "mysql" {
        return Err(DbHostError::UnsupportedEngine(engine));
    }
    is_readonly_sql(sql)?;
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MYSQL_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MYSQL_EXPLORER_URL not set".into()))?;
    let cap = (max_rows as usize).min(5_000);
    let q = sql.trim();
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(&url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let rows = sqlx::query(q)
        .fetch_all(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
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
    Ok(HostDatabaseQueryResultView {
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

/// Same as [`mysql_query`] but uses an already-built connection string (per-request creds).
pub async fn mysql_query_for_url(
    url: &str,
    sql: &str,
    max_rows: u32,
) -> Result<HostDatabaseQueryResultView, DbHostError> {
    is_readonly_sql(sql)?;
    let cap = (max_rows as usize).min(5_000);
    let q = sql.trim();
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let rows = sqlx::query(q)
        .fetch_all(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
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
    Ok(HostDatabaseQueryResultView {
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

pub async fn pg_schemas_view(pool: &PgPool) -> Result<DatabaseSchemasView, DbHostError> {
    let schemas = explorer_schemas(pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(DatabaseSchemasView {
        configured: true,
        schemas,
    })
}

pub async fn pg_tables_view(
    pool: &PgPool,
    schema: &str,
) -> Result<DatabaseTablesView, DbHostError> {
    let tables = explorer_tables(pool, schema)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(DatabaseTablesView {
        configured: true,
        schema: schema.to_string(),
        tables,
    })
}

pub async fn pg_columns_view(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<DatabaseColumnsView, DbHostError> {
    let columns = explorer_columns(pool, schema, table)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(DatabaseColumnsView {
        configured: true,
        schema: schema.to_string(),
        table: table.to_string(),
        columns,
    })
}

pub async fn pg_rel_view(pool: &PgPool) -> Result<DatabaseRelationshipsView, DbHostError> {
    let foreign_keys = explorer_foreign_keys(pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(DatabaseRelationshipsView {
        configured: true,
        foreign_keys,
    })
}

pub async fn pg_preview(
    pool: &PgPool,
    schema: &str,
    table: &str,
    limit: i64,
    offset: i64,
) -> Result<DatabaseTablePreviewView, DbHostError> {
    let preview = explorer_table_preview(pool, schema, table, limit, offset)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(DatabaseTablePreviewView {
        configured: true,
        schema: schema.to_string(),
        table: table.to_string(),
        preview: Some(preview),
    })
}

fn clickhouse_base_http_from_env(m: &HashMap<String, String>) -> Result<String, DbHostError> {
    let base = env_get(m, "PIRATE_CLICKHOUSE_HTTP_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_CLICKHOUSE_HTTP_URL not set".into()))?;
    let _ = Url::parse(&base).map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(base.trim_end_matches('/').to_string())
}

/// ClickHouse over HTTP, JSON response (`base` is e.g. `http://h:8123` or with userinfo).
pub async fn clickhouse_query_http(
    base: &str,
    sql: &str,
    max_rows: u32,
) -> Result<HostDatabaseQueryResultView, DbHostError> {
    is_readonly_sql(sql)?;
    let client = reqwest::Client::builder()
        .timeout(QUERY_TIMEOUT)
        .build()
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let u = Url::parse(&format!("{base}/"))
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let user = u.username();
    let pass = u.password().unwrap_or("");
    let user_empty = user.is_empty();
    let body = format!(
        "{} LIMIT {}",
        sql.trim().trim_end_matches(';'),
        max_rows.min(5000)
    );
    let mut req = client
        .post(format!("{}/", base.trim_end_matches('/')))
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(body);
    if !user_empty {
        req = req.basic_auth(user, Some(pass));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(DbHostError::Backend(t));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| json!({ "raw": text }));
    let (mut cols, rows): (Vec<String>, Vec<serde_json::Value>) =
        if let (Some(data), Some(meta)) = (val.get("data"), val.get("meta")) {
            if let (Some(da), Some(me)) = (data.as_array(), meta.as_array()) {
                let mut c = vec![];
                for m in me {
                    if let Some(n) = m.get("name").and_then(|x| x.as_str()) {
                        c.push(n.to_string());
                    }
                }
                (c, da.clone())
            } else {
                (vec![], vec![])
            }
        } else {
            (vec![], vec![json!({ "value": val })])
        };
    if cols.is_empty() && !rows.is_empty() {
        if let Some(r0) = rows.first().and_then(|r| r.as_object()) {
            cols = r0.keys().cloned().collect();
        }
    }
    let row_count = rows.len();
    Ok(HostDatabaseQueryResultView {
        columns: cols,
        rows: rows
            .into_iter()
            .map(|c| c.clone())
            .collect(),
        row_count,
        truncated: false,
        warn: None,
    })
}

/// ClickHouse using `PIRATE_CLICKHOUSE_HTTP_URL` from the host env file.
pub async fn clickhouse_query(
    env_path: &Path,
    _instance_id: &str,
    sql: &str,
    max_rows: u32,
) -> Result<HostDatabaseQueryResultView, DbHostError> {
    let m = read_host_env_file(env_path);
    let base = clickhouse_base_http_from_env(&m)?;
    clickhouse_query_http(&base, sql, max_rows).await
}

pub async fn redis_keys_for_url(
    url: &str,
    pattern: &str,
    cursor: &str,
) -> Result<HostDatabaseRedisKeysView, DbHostError> {
    let client = redis::Client::open(url).map_err(|e| DbHostError::Backend(e.to_string()))?;
    let mut conn = redis::aio::ConnectionManager::new(client)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let cur: u64 = if cursor == "0" || cursor.is_empty() {
        0
    } else {
        cursor.parse().unwrap_or(0)
    };
    let pat = if pattern.is_empty() { "*" } else { pattern };
    let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
        .arg(cur)
        .arg("MATCH")
        .arg(pat)
        .arg("COUNT")
        .arg(50u32)
        .query_async(&mut conn)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let mut out = Vec::new();
    for k in keys.into_iter().take(MAX_SCAN_KEYS) {
        let typ: String = redis::cmd("TYPE")
            .arg(&k)
            .query_async(&mut conn)
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        out.push(HostDatabaseRedisKeyView {
            key: k,
            type_name: Some(typ),
            ttl_sec: None,
        });
    }
    let done = next == 0;
    let next_s = if done { "0".into() } else { next.to_string() };
    Ok(HostDatabaseRedisKeysView {
        keys: out,
        cursor: next_s,
        done,
    })
}

pub async fn redis_keys(
    env_path: &Path,
    _instance_id: &str,
    pattern: &str,
    cursor: &str,
) -> Result<HostDatabaseRedisKeysView, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_REDIS_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_REDIS_URL not set".into()))?;
    redis_keys_for_url(&url, pattern, cursor).await
}

fn validate_mongo_ident(part: &str) -> Result<(), DbHostError> {
    if part.is_empty()
        || !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(DbHostError::SqlPolicy("invalid MongoDB identifier".into()));
    }
    Ok(())
}

async fn mongosh_eval(url: &str, js: &str) -> Result<String, DbHostError> {
    let out = tokio::process::Command::new("mongosh")
        .arg(url)
        .arg("--quiet")
        .arg("--eval")
        .arg(js)
        .output()
        .await
        .map_err(|e| DbHostError::Backend(format!("mongosh: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(DbHostError::Backend(
            if err.trim().is_empty() {
                "mongosh command failed".into()
            } else {
                err.trim().to_string()
            },
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub async fn mongo_databases_for_url(url: &str) -> Result<Vec<String>, DbHostError> {
    let s = mongosh_eval(
        url,
        "JSON.stringify(db.getMongo().getDBNames())",
    )
    .await?;
    serde_json::from_str(&s).map_err(|e| {
        DbHostError::Backend(format!("mongosh JSON parse: {e}; output: {s}"))
    })
}

pub async fn mongo_databases(env_path: &Path) -> Result<Vec<String>, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MONGO_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MONGO_EXPLORER_URL not set".into()))?;
    mongo_databases_for_url(&url).await
}

pub async fn mongo_collections_for_url(url: &str, db_name: &str) -> Result<Vec<String>, DbHostError> {
    validate_mongo_ident(db_name)?;
    let js = format!(
        r#"JSON.stringify(db.getSiblingDB("{}").getCollectionNames())"#,
        db_name
    );
    let s = mongosh_eval(url, &js).await?;
    serde_json::from_str(&s).map_err(|e| {
        DbHostError::Backend(format!("mongosh JSON parse: {e}; output: {s}"))
    })
}

pub async fn mongo_collections(
    env_path: &Path,
    db_name: &str,
) -> Result<Vec<String>, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MONGO_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MONGO_EXPLORER_URL not set".into()))?;
    mongo_collections_for_url(&url, db_name).await
}

pub async fn mongo_preview_for_url(
    url: &str,
    db_name: &str,
    collection: &str,
    limit: u32,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    validate_mongo_ident(db_name)?;
    validate_mongo_ident(collection)?;
    let lim = limit.min(200) as u64;
    let js = format!(
        r#"JSON.stringify(db.getSiblingDB("{db}").getCollection("{coll}").find().limit({lim}).toArray())"#,
        db = db_name,
        coll = collection,
        lim = lim
    );
    let s = mongosh_eval(url, &js).await?;
    serde_json::from_str(&s).map_err(|e| {
        DbHostError::Backend(format!("mongosh JSON parse: {e}; output: {s}"))
    })
}

pub async fn mongo_preview(
    env_path: &Path,
    db_name: &str,
    collection: &str,
    limit: u32,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MONGO_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MONGO_EXPLORER_URL not set".into()))?;
    mongo_preview_for_url(&url, db_name, collection, limit).await
}

// --- MySQL information_schema helpers ---

pub async fn mysql_schemas_for_url(
    url: &str,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let rows: Vec<(String,)> = sqlx::query_as("SELECT schema_name FROM information_schema.schemata ORDER BY schema_name")
        .fetch_all(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(s,)| json!({ "name": s }))
        .collect())
}

pub async fn mysql_schemas(
    env_path: &Path,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MYSQL_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MYSQL_EXPLORER_URL not set".into()))?;
    mysql_schemas_for_url(&url).await
}

pub async fn mysql_tables_for_url(
    url: &str,
    schema: &str,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_schema = ? ORDER BY table_name"
    )
    .bind(schema)
    .fetch_all(&pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(schema_name, name, table_type)| {
            json!({
                "schema_name": schema_name,
                "name": name,
                "table_type": table_type
            })
        })
        .collect())
}

pub async fn mysql_tables(
    env_path: &Path,
    schema: &str,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MYSQL_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MYSQL_EXPLORER_URL not set".into()))?;
    mysql_tables_for_url(&url, schema).await
}

pub async fn mysql_columns_for_url(
    url: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let rows: Vec<(
        String,
        String,
        String,
        String,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT table_schema, table_name, column_name, data_type, column_default
         FROM information_schema.columns WHERE table_schema = ? AND table_name = ? ORDER BY ordinal_position",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(&pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(
            |(table_schema, table_name, column_name, data_type, column_default)| {
                json!({
                    "table_schema": table_schema,
                    "table_name": table_name,
                    "column_name": column_name,
                    "data_type": data_type,
                    "column_default": column_default
                })
            },
        )
        .collect())
}

pub async fn mysql_columns(
    env_path: &Path,
    schema: &str,
    table: &str,
) -> Result<Vec<serde_json::Value>, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MYSQL_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MYSQL_EXPLORER_URL not set".into()))?;
    mysql_columns_for_url(&url, schema, table).await
}

pub async fn mysql_preview_for_url(
    url: &str,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
) -> Result<serde_json::Value, DbHostError> {
    for s in [schema, table] {
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(DbHostError::SqlPolicy("invalid identifier".into()));
        }
    }
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let q = format!(
        "SELECT * FROM `{}`.`{}` LIMIT {} OFFSET {}",
        schema,
        table,
        limit.min(5000),
        offset
    );
    let rows = sqlx::query(&q)
        .fetch_all(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let mut out = vec![];
    for r in rows {
        out.push(mysql_row_to_json(&r));
    }
    Ok(json!({ "rows": out }))
}

pub async fn mysql_preview(
    env_path: &Path,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
) -> Result<serde_json::Value, DbHostError> {
    let m = read_host_env_file(env_path);
    let url = env_get(&m, "PIRATE_MYSQL_EXPLORER_URL")
        .ok_or_else(|| DbHostError::Backend("PIRATE_MYSQL_EXPLORER_URL not set".into()))?;
    mysql_preview_for_url(&url, schema, table, limit, offset).await
}

fn json_to_sql_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        serde_json::Value::Null => "NULL".into(),
        _ => format!("'{}'", v.to_string().replace('\'', "''")),
    }
}

fn mysql_ident_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Server-side filtered/sorted table browse (PostgreSQL).
pub async fn pg_v2_grid(
    pool: &PgPool,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
    sort_column: Option<&str>,
    sort_desc: bool,
    filter_column: Option<&str>,
    filter_value: Option<&serde_json::Value>,
) -> Result<(HostDatabaseQueryResultView, Option<usize>), DbHostError> {
    validate_pg_ident(schema).map_err(|e| DbHostError::Backend(e.to_string()))?;
    validate_pg_ident(table).map_err(|e| DbHostError::Backend(e.to_string()))?;
    if let Some(c) = sort_column {
        validate_pg_ident(c).map_err(|e| DbHostError::Backend(e.to_string()))?;
    }
    if let Some(c) = filter_column {
        validate_pg_ident(c).map_err(|e| DbHostError::Backend(e.to_string()))?;
    }
    let sch_q = format!("\"{}\"", schema.replace('"', ""));
    let tbl_q = format!("\"{}\"", table.replace('"', ""));
    let mut where_sql = String::new();
    if let (Some(fc), Some(fv)) = (filter_column, filter_value) {
        let col = format!("\"{}\"", fc.replace('"', ""));
        where_sql = format!(" WHERE {} = {}", col, json_to_sql_literal(fv));
    }
    let order_sql = if let Some(sc) = sort_column {
        format!(
            " ORDER BY \"{}\" {}",
            sc.replace('"', ""),
            if sort_desc { "DESC" } else { "ASC" }
        )
    } else {
        String::new()
    };
    let lim = limit.min(5000) as i64;
    let off = offset as i64;
    let sql = format!(
        "SELECT * FROM {}.{}{}{} LIMIT {} OFFSET {}",
        sch_q, tbl_q, where_sql, order_sql, lim, off
    );
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let mut cols: Vec<String> = Vec::new();
    if let Some(r) = rows.first() {
        cols = r.columns().iter().map(|c| c.name().to_string()).collect();
    }
    let mut out = Vec::new();
    for r in rows {
        out.push(pg_row_to_json(&r));
    }
    let count_sql = format!("SELECT count(*)::bigint AS c FROM {}.{}{}", sch_q, tbl_q, where_sql);
    let cnt: i64 = sqlx::query_scalar(&count_sql)
        .fetch_one(pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let res = HostDatabaseQueryResultView {
        columns: cols,
        row_count: out.len(),
        rows: out,
        truncated: false,
        warn: None,
    };
    Ok((res, Some(cnt.max(0) as usize)))
}

/// Server-side filtered/sorted table browse (MySQL).
pub async fn mysql_v2_grid(
    url: &str,
    schema: &str,
    table: &str,
    limit: u32,
    offset: u32,
    sort_column: Option<&str>,
    sort_desc: bool,
    filter_column: Option<&str>,
    filter_value: Option<&serde_json::Value>,
) -> Result<(HostDatabaseQueryResultView, Option<usize>), DbHostError> {
    for s in [schema, table] {
        if !mysql_ident_ok(s) {
            return Err(DbHostError::SqlPolicy("invalid identifier".into()));
        }
    }
    if let Some(c) = sort_column {
        if !mysql_ident_ok(c) {
            return Err(DbHostError::SqlPolicy("invalid sort column".into()));
        }
    }
    if let Some(c) = filter_column {
        if !mysql_ident_ok(c) {
            return Err(DbHostError::SqlPolicy("invalid filter column".into()));
        }
    }
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let mut where_sql = String::new();
    if let (Some(fc), Some(fv)) = (filter_column, filter_value) {
        where_sql = format!(
            " WHERE `{}` = {}",
            fc.replace('`', ""),
            json_to_sql_literal(fv)
        );
    }
    let order_sql = if let Some(sc) = sort_column {
        format!(
            " ORDER BY `{}` {}",
            sc.replace('`', ""),
            if sort_desc { "DESC" } else { "ASC" }
        )
    } else {
        String::new()
    };
    let lim = limit.min(5000);
    let sql = format!(
        "SELECT * FROM `{}`.`{}`{}{} LIMIT {} OFFSET {}",
        schema, table, where_sql, order_sql, lim, offset
    );
    let rows = sqlx::query(&sql)
        .fetch_all(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let mut cols: Vec<String> = Vec::new();
    if let Some(r) = rows.first() {
        cols = r.columns().iter().map(|c| c.name().to_string()).collect();
    }
    let mut out = Vec::new();
    for r in rows {
        out.push(mysql_row_to_json(&r));
    }
    let count_sql = format!(
        "SELECT count(*) AS c FROM `{}`.`{}`{}",
        schema, table, where_sql
    );
    let cnt: i64 = sqlx::query_scalar(&count_sql)
        .fetch_one(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let res = HostDatabaseQueryResultView {
        columns: cols,
        row_count: out.len(),
        rows: out,
        truncated: false,
        warn: None,
    };
    Ok((res, Some(cnt.max(0) as usize)))
}

/// Structured row write (PostgreSQL). `op`: `insert` | `update` | `delete`.
pub async fn pg_row_mutate(
    pool: &PgPool,
    op: &str,
    schema: &str,
    table: &str,
    pk: Option<&serde_json::Map<String, serde_json::Value>>,
    row: &serde_json::Value,
) -> Result<u64, DbHostError> {
    validate_pg_ident(schema).map_err(|e| DbHostError::Backend(e.to_string()))?;
    validate_pg_ident(table).map_err(|e| DbHostError::Backend(e.to_string()))?;
    let sch_q = format!("\"{}\"", schema.replace('"', ""));
    let tbl_q = format!("\"{}\"", table.replace('"', ""));
    let op = op.to_ascii_lowercase();
    match op.as_str() {
        "delete" => {
            let pk = pk.ok_or_else(|| DbHostError::SqlPolicy("delete requires pk".into()))?;
            if pk.is_empty() {
                return Err(DbHostError::SqlPolicy("empty pk".into()));
            }
            let mut w = Vec::new();
            for (k, v) in pk.iter() {
                validate_pg_ident(k).map_err(|e| DbHostError::Backend(e.to_string()))?;
                w.push(format!("\"{}\" = {}", k, json_to_sql_literal(v)));
            }
            let sql = format!(
                "DELETE FROM {}.{} WHERE {}",
                sch_q,
                tbl_q,
                w.join(" AND ")
            );
            let r = sqlx::query(&sql)
                .execute(pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            Ok(r.rows_affected())
        }
        "update" => {
            let pk = pk.ok_or_else(|| DbHostError::SqlPolicy("update requires pk".into()))?;
            let obj = row.as_object().ok_or_else(|| DbHostError::SqlPolicy("row must be object".into()))?;
            if obj.is_empty() {
                return Err(DbHostError::SqlPolicy("empty row".into()));
            }
            let mut sets = Vec::new();
            for (k, v) in obj.iter() {
                validate_pg_ident(k).map_err(|e| DbHostError::Backend(e.to_string()))?;
                sets.push(format!("\"{}\" = {}", k, json_to_sql_literal(v)));
            }
            let mut w = Vec::new();
            for (k, v) in pk.iter() {
                validate_pg_ident(k).map_err(|e| DbHostError::Backend(e.to_string()))?;
                w.push(format!("\"{}\" = {}", k, json_to_sql_literal(v)));
            }
            let sql = format!(
                "UPDATE {}.{} SET {} WHERE {}",
                sch_q,
                tbl_q,
                sets.join(", "),
                w.join(" AND ")
            );
            let r = sqlx::query(&sql)
                .execute(pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            Ok(r.rows_affected())
        }
        "insert" => {
            let obj = row.as_object().ok_or_else(|| DbHostError::SqlPolicy("row must be object".into()))?;
            if obj.is_empty() {
                return Err(DbHostError::SqlPolicy("empty row".into()));
            }
            let mut cols = Vec::new();
            let mut vals = Vec::new();
            for (k, v) in obj.iter() {
                validate_pg_ident(k).map_err(|e| DbHostError::Backend(e.to_string()))?;
                cols.push(format!("\"{}\"", k));
                vals.push(json_to_sql_literal(v));
            }
            let sql = format!(
                "INSERT INTO {}.{} ({}) VALUES ({})",
                sch_q,
                tbl_q,
                cols.join(", "),
                vals.join(", ")
            );
            let r = sqlx::query(&sql)
                .execute(pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            Ok(r.rows_affected())
        }
        _ => Err(DbHostError::SqlPolicy("op must be insert|update|delete".into())),
    }
}

/// Structured row write (MySQL).
pub async fn mysql_row_mutate(
    url: &str,
    op: &str,
    schema: &str,
    table: &str,
    pk: Option<&serde_json::Map<String, serde_json::Value>>,
    row: &serde_json::Value,
) -> Result<u64, DbHostError> {
    for s in [schema, table] {
        if !mysql_ident_ok(s) {
            return Err(DbHostError::SqlPolicy("invalid identifier".into()));
        }
    }
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(QUERY_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let op = op.to_ascii_lowercase();
    match op.as_str() {
        "delete" => {
            let pk = pk.ok_or_else(|| DbHostError::SqlPolicy("delete requires pk".into()))?;
            let mut w = Vec::new();
            for (k, v) in pk.iter() {
                if !mysql_ident_ok(k) {
                    return Err(DbHostError::SqlPolicy("invalid pk col".into()));
                }
                w.push(format!("`{}` = {}", k.replace('`', ""), json_to_sql_literal(v)));
            }
            let sql = format!(
                "DELETE FROM `{}`.`{}` WHERE {}",
                schema, table, w.join(" AND ")
            );
            let r = sqlx::query(&sql)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            Ok(r.rows_affected())
        }
        "update" => {
            let pk = pk.ok_or_else(|| DbHostError::SqlPolicy("update requires pk".into()))?;
            let obj = row.as_object().ok_or_else(|| DbHostError::SqlPolicy("row must be object".into()))?;
            let mut sets = Vec::new();
            for (k, v) in obj.iter() {
                if !mysql_ident_ok(k) {
                    return Err(DbHostError::SqlPolicy("invalid column".into()));
                }
                sets.push(format!("`{}` = {}", k.replace('`', ""), json_to_sql_literal(v)));
            }
            let mut w = Vec::new();
            for (k, v) in pk.iter() {
                if !mysql_ident_ok(k) {
                    return Err(DbHostError::SqlPolicy("invalid pk col".into()));
                }
                w.push(format!("`{}` = {}", k.replace('`', ""), json_to_sql_literal(v)));
            }
            let sql = format!(
                "UPDATE `{}`.`{}` SET {} WHERE {}",
                schema,
                table,
                sets.join(", "),
                w.join(" AND ")
            );
            let r = sqlx::query(&sql)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            Ok(r.rows_affected())
        }
        "insert" => {
            let obj = row.as_object().ok_or_else(|| DbHostError::SqlPolicy("row must be object".into()))?;
            let mut cols = Vec::new();
            let mut vals = Vec::new();
            for (k, v) in obj.iter() {
                if !mysql_ident_ok(k) {
                    return Err(DbHostError::SqlPolicy("invalid column".into()));
                }
                cols.push(format!("`{}`", k.replace('`', "")));
                vals.push(json_to_sql_literal(v));
            }
            let sql = format!(
                "INSERT INTO `{}`.`{}` ({}) VALUES ({})",
                schema,
                table,
                cols.join(", "),
                vals.join(", ")
            );
            let r = sqlx::query(&sql)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            Ok(r.rows_affected())
        }
        _ => Err(DbHostError::SqlPolicy("op must be insert|update|delete".into())),
    }
}

#[cfg(test)]
mod readonly_sql_tests {
    use super::is_readonly_sql;

    #[test]
    fn allows_select() {
        is_readonly_sql("SELECT 1").unwrap();
    }

    #[test]
    fn rejects_insert() {
        assert!(is_readonly_sql("INSERT INTO t VALUES (1)").is_err());
    }
}