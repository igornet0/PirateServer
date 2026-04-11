//! Read-only PostgreSQL schema explorer (validated identifiers only; no arbitrary SQL from clients).

use crate::DbError;
use serde::Serialize;
use sqlx::postgres::PgPool;
use sqlx::FromRow;

/// PostgreSQL identifier: letters, digits, underscore; must start with letter or `_`; max 63 chars.
pub fn validate_pg_ident(s: &str) -> Result<(), DbError> {
    if s.is_empty() || s.len() > 63 {
        return Err(DbError::InvalidIdentifier(s.to_string()));
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(DbError::InvalidIdentifier(s.to_string()));
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(DbError::InvalidIdentifier(s.to_string()));
    }
    Ok(())
}

fn quote_ident(s: &str) -> Result<String, DbError> {
    validate_pg_ident(s)?;
    Ok(format!("\"{}\"", s.replace('"', "")))
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct SchemaRow {
    pub name: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct TableSummaryRow {
    pub schema_name: String,
    pub name: String,
    pub table_type: String,
    pub row_estimate: Option<i64>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct TableColumnRow {
    pub column_name: String,
    pub data_type: String,
    pub is_nullable: String,
    pub column_default: Option<String>,
    pub character_maximum_length: Option<i32>,
    pub numeric_precision: Option<i32>,
    pub numeric_scale: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TablePreview {
    pub rows: serde_json::Value,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ForeignKeyRow {
    pub table_schema: String,
    pub table_name: String,
    pub column_name: String,
    pub foreign_table_schema: String,
    pub foreign_table_name: String,
    pub foreign_column_name: String,
    pub constraint_name: String,
}

/// Metrics from the live PostgreSQL session (read-only queries).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PostgresServerInfoRow {
    pub server_version: String,
    pub database_name: String,
    pub session_user: String,
    pub database_size_bytes: i64,
    pub active_connections: i64,
}

pub async fn fetch_postgres_server_info(pool: &PgPool) -> Result<PostgresServerInfoRow, DbError> {
    let row = sqlx::query_as::<_, PostgresServerInfoRow>(
        r#"
            SELECT
              version() AS server_version,
              current_database() AS database_name,
              current_user AS session_user,
              pg_database_size(current_database()) AS database_size_bytes,
              (
                SELECT count(*)::bigint
                FROM pg_stat_activity
                WHERE datname = current_database()
              ) AS active_connections
            "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn explorer_schemas(pool: &PgPool) -> Result<Vec<SchemaRow>, DbError> {
    let rows = sqlx::query_as::<_, SchemaRow>(
        r#"
            SELECT schema_name AS name
            FROM information_schema.schemata
            WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
              AND schema_name NOT LIKE 'pg\_%' ESCAPE '\'
            ORDER BY schema_name
            "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn explorer_tables(pool: &PgPool, schema: &str) -> Result<Vec<TableSummaryRow>, DbError> {
    validate_pg_ident(schema)?;
    let rows = sqlx::query_as::<_, TableSummaryRow>(
        r#"
            SELECT
              t.table_schema::text AS schema_name,
              t.table_name::text AS name,
              t.table_type::text AS table_type,
              CASE WHEN t.table_type = 'BASE TABLE' THEN s.n_live_tup::bigint ELSE NULL END AS row_estimate
            FROM information_schema.tables t
            LEFT JOIN pg_stat_user_tables s
              ON s.schemaname = t.table_schema AND s.relname = t.table_name
            WHERE t.table_schema = $1
              AND t.table_type IN ('BASE TABLE', 'VIEW')
            ORDER BY t.table_name
            "#,
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn explorer_columns(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<Vec<TableColumnRow>, DbError> {
    validate_pg_ident(schema)?;
    validate_pg_ident(table)?;
    let rows = sqlx::query_as::<_, TableColumnRow>(
        r#"
            SELECT
              column_name::text,
              data_type::text,
              is_nullable::text,
              column_default::text,
              character_maximum_length,
              numeric_precision,
              numeric_scale
            FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = $2
            ORDER BY ordinal_position
            "#,
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn explorer_foreign_keys(pool: &PgPool) -> Result<Vec<ForeignKeyRow>, DbError> {
    let rows = sqlx::query_as::<_, ForeignKeyRow>(
        r#"
            SELECT
              tc.table_schema::text,
              tc.table_name::text,
              kcu.column_name::text,
              ccu.table_schema::text AS foreign_table_schema,
              ccu.table_name::text AS foreign_table_name,
              ccu.column_name::text AS foreign_column_name,
              tc.constraint_name::text
            FROM information_schema.table_constraints AS tc
            JOIN information_schema.key_column_usage AS kcu
              ON tc.constraint_catalog = kcu.constraint_catalog
              AND tc.constraint_schema = kcu.constraint_schema
              AND tc.constraint_name = kcu.constraint_name
            JOIN information_schema.constraint_column_usage AS ccu
              ON ccu.constraint_catalog = tc.constraint_catalog
              AND ccu.constraint_schema = tc.constraint_schema
              AND ccu.constraint_name = tc.constraint_name
            WHERE tc.constraint_type = 'FOREIGN KEY'
              AND tc.table_schema NOT IN ('pg_catalog', 'information_schema')
            ORDER BY tc.table_schema, tc.table_name, kcu.ordinal_position
            "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Returns rows as a JSON array of objects (max `limit` rows, `offset` for paging).
pub async fn explorer_table_preview(
    pool: &PgPool,
    schema: &str,
    table: &str,
    limit: i64,
    offset: i64,
) -> Result<TablePreview, DbError> {
    validate_pg_ident(schema)?;
    validate_pg_ident(table)?;
    let lim = limit.clamp(1, 500);
    let off = offset.max(0);
    let qs = quote_ident(schema)?;
    let qt = quote_ident(table)?;
    let sql = format!(
        r#"SELECT COALESCE(json_agg(row_to_json(q)), '[]'::json)
               FROM (SELECT * FROM {}.{} LIMIT $1 OFFSET $2) q"#,
        qs, qt
    );
    let rows: serde_json::Value = sqlx::query_scalar::<_, serde_json::Value>(&sql)
        .bind(lim)
        .bind(off)
        .fetch_one(pool)
        .await?;
    Ok(TablePreview { rows })
}
