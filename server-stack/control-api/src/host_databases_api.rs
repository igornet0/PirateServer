//! Multi-engine host database discovery and read-only browser (`/api/v1/host-databases/*`).
//! Policy: [`crate::ApiState`] env caps, audit logs without raw SQL, optional disable via
//! `CONTROL_API_HOST_DATABASES=0` (ops «RBAC» — authenticated JWT users only, plus this gate).
use axum::extract::{Path, Query, State};
use axum::Json;
use deploy_control::{
    HostDatabaseQueryBody, HostDatabaseQueryResultView, HostDatabaseRedisKeysView,
    HostDatabasesListView, HostDbRequestCredentials,
};
use serde::Deserialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::ApiState;
use axum::http::HeaderMap;

const PIRATE_DB_USER_HEADER: &str = "x-pirate-db-user";
const PIRATE_DB_PASSWORD_HEADER: &str = "x-pirate-db-password";

/// Per-request DB credentials (never logged). Password may be empty for some auth modes.
pub(crate) fn host_db_creds_from_headers(headers: &HeaderMap) -> Option<HostDbRequestCredentials> {
    let u = headers.get(PIRATE_DB_USER_HEADER)?.to_str().ok()?;
    if u.trim().is_empty() {
        return None;
    }
    let p = headers
        .get(PIRATE_DB_PASSWORD_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    Some(HostDbRequestCredentials {
        user: u.to_string(),
        pass: p.to_string(),
    })
}

fn check_host_databases(s: &ApiState) -> Result<(), ApiError> {
    if s.host_databases_enabled {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "host database API is disabled (set CONTROL_API_HOST_DATABASES=1 to enable)",
        ))
    }
}

/// Single-line, length-limited fingerprint for logs (never log full `sql`: may contain literals).
pub(crate) fn sql_fingerprint_for_audit(sql: &str) -> String {
    let t = sql.trim();
    let one_line: String = t.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
    let max = 160usize;
    if one_line.chars().count() <= max {
        one_line
    } else {
        one_line.chars().take(max).collect::<String>() + "…"
    }
}

fn audit_host_op(op: &str, instance_id: &str, detail: &str) {
    tracing::info!(
        target: "pirate.db.audit",
        op = op,
        instance = %instance_id,
        detail = %detail,
        "host database op"
    );
}

pub async fn api_host_databases_list(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<HostDatabasesListView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    let out = s
        .plane
        .host_databases_list()
        .await
        .map_err(ApiError::from)?;
    audit_host_op("list", "*", "instances");
    Ok(Json(out))
}

pub async fn api_host_db_schemas(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    audit_host_op("schemas", &instance_id, "");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_schemas_json(&instance_id, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize)]
pub struct HostDbSchemaQ {
    pub schema: String,
}

pub async fn api_host_db_tables(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Query(q): Query<HostDbSchemaQ>,
) -> Result<Json<Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    audit_host_op("tables", &instance_id, "schema=…");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_tables_json(&instance_id, &q.schema, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

pub async fn api_host_db_columns(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path((instance_id, schema, table)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    audit_host_op("columns", &instance_id, "table");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_columns_json(&instance_id, &schema, &table, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize)]
pub struct HostDbRowsQ {
    pub schema: String,
    pub table: String,
    #[serde(default = "default_rows_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_rows_limit() -> u32 {
    100
}

pub async fn api_host_db_rows(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Query(q): Query<HostDbRowsQ>,
) -> Result<Json<Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    let limit = q
        .limit
        .clamp(1, s.host_db_max_preview_limit);
    let offset = q.offset.min(s.host_db_max_offset);
    let detail = format!("limit={limit} offset={offset}");
    audit_host_op("rows", &instance_id, &detail);
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_rows_json(
            &instance_id,
            &q.schema,
            &q.table,
            limit,
            offset,
            creds.as_ref(),
        )
        .await
        .map(Json)
        .map_err(Into::into)
}

pub async fn api_host_db_relationships(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    audit_host_op("relationships", &instance_id, "");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_relationships_json(&instance_id, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

pub async fn api_host_db_query(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(mut body): Json<HostDatabaseQueryBody>,
) -> Result<Json<HostDatabaseQueryResultView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    if body.sql.len() > s.host_db_max_sql_bytes {
        return Err(ApiError::bad_request(format!(
            "SQL exceeds max length ({} > {}); raise CONTROL_API_HOST_DB_MAX_SQL_BYTES if needed",
            body.sql.len(),
            s.host_db_max_sql_bytes
        )));
    }
    if body.max_rows == 0 {
        body.max_rows = 500;
    }
    body.max_rows = body.max_rows.clamp(1, s.host_db_max_query_rows);
    let fp = sql_fingerprint_for_audit(&body.sql);
    tracing::info!(
        target: "pirate.db.audit",
        op = "query",
        instance = %instance_id,
        sql_len = body.sql.len(),
        max_rows = body.max_rows,
        sql_fingerprint = %fp,
        "host database query (fingerprint only; not full SQL)"
    );
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_query(&instance_id, &body, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize)]
pub struct RedisKeysQ {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub cursor: String,
}

pub async fn api_host_db_redis_keys(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Query(q): Query<RedisKeysQ>,
) -> Result<Json<HostDatabaseRedisKeysView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    if q.pattern.len() > s.host_db_max_redis_pattern_bytes {
        return Err(ApiError::bad_request(format!(
            "redis pattern too long (max {} bytes)",
            s.host_db_max_redis_pattern_bytes
        )));
    }
    audit_host_op("redis_keys", &instance_id, "scan");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_redis_keys(&instance_id, &q.pattern, &q.cursor, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

pub async fn api_host_db_mongo_databases(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    audit_host_op("mongo_databases", &instance_id, "");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_mongo_dbs(&instance_id, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize)]
pub struct MongoCollQ {
    pub db: String,
}

pub async fn api_host_db_mongo_collections(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Query(q): Query<MongoCollQ>,
) -> Result<Json<Vec<String>>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    audit_host_op("mongo_collections", &instance_id, "db=…");
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_mongo_collections(&instance_id, &q.db, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize)]
pub struct MongoPreviewQ {
    pub db: String,
    pub collection: String,
    #[serde(default = "default_mongo_limit")]
    pub limit: u32,
}

fn default_mongo_limit() -> u32 {
    50
}

pub async fn api_host_db_mongo_preview(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Query(q): Query<MongoPreviewQ>,
) -> Result<Json<Vec<Value>>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases(&s)?;
    let limit = q.limit.clamp(1, s.host_db_max_preview_limit);
    audit_host_op("mongo_preview", &instance_id, &format!("limit={limit}"));
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_mongo_preview(&instance_id, &q.db, &q.collection, limit, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::sql_fingerprint_for_audit;

    #[test]
    fn fingerprint_truncates_long_sql() {
        let s: String = (0..300).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let f = sql_fingerprint_for_audit(&s);
        assert!(f.len() < s.len());
        assert!(f.ends_with('…'));
    }

    #[test]
    fn fingerprint_strips_newlines() {
        let f = sql_fingerprint_for_audit("SELECT\n1");
        assert!(!f.contains('\n'));
        assert_eq!(f, "SELECT1");
    }
}