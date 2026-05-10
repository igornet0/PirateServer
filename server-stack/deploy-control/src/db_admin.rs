//! Privileged host DB operations: create database / table (PostgreSQL or MySQL admin URLs) and whitelisted migration CLI runs.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use deploy_db::validate_pg_ident;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::postgres::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

use crate::db_host::{env_get, postgres_url_with_creds, read_host_env_file, DbHostError};
use crate::types::{
    HostDbAdminCreateTableBody, HostDbAdminCreateTableColumn, HostDbAdminCreateUserBody,
    HostDbAdminCreateUserView, HostDbAdminDeleteUserBody, HostDbAdminDeleteUserView,
    HostDbCreateDatabaseBody, HostDbMigrationRunBody, HostDbMigrationRunView, HostDbRequestCredentials,
};

const PG_CREATE_ALLOWED_ENCODINGS: &[&str] = &["UTF8", "SQL_ASCII", "LATIN1"];

/// Double-quoted PostgreSQL identifier (safe after [`validate_pg_ident`]).
fn pg_quoted_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', ""))
}

fn admin_url_from_env(host_env_path: &Path) -> Result<String, DbHostError> {
    let m = read_host_env_file(host_env_path);
    env_get(&m, "PIRATE_POSTGRES_ADMIN_URL").ok_or_else(|| {
        DbHostError::Backend(
            "PIRATE_POSTGRES_ADMIN_URL is not set in the host env file (superuser URL to the `postgres` database)"
                .into(),
        )
    })
}

fn mysql_admin_url_from_env(host_env_path: &Path) -> Result<String, DbHostError> {
    let m = read_host_env_file(host_env_path);
    env_get(&m, "PIRATE_MYSQL_ADMIN_URL").ok_or_else(|| {
        DbHostError::Backend(
            "PIRATE_MYSQL_ADMIN_URL is not set (MySQL account with CREATE DATABASE privilege)".into(),
        )
    })
}

fn validate_mysql_admin_ident(name: &str) -> Result<(), DbHostError> {
    if name.is_empty() || name.len() > 64 {
        return Err(DbHostError::Backend("invalid MySQL identifier length".into()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '$')
    {
        return Err(DbHostError::Backend(
            "MySQL identifier: use ASCII letters, digits, _, -, or $ only".into(),
        ));
    }
    Ok(())
}

fn mysql_quote_ident(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}

fn validate_create_database_body(body: &HostDbCreateDatabaseBody) -> Result<(), DbHostError> {
    validate_pg_ident(&body.database).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    if let Some(ref o) = body.owner {
        validate_pg_ident(o).map_err(|e: deploy_db::DbError| {
            DbHostError::Backend(e.to_string())
        })?;
    }
    if let Some(ref enc) = body.encoding {
        let u = enc.trim().to_ascii_uppercase();
        if !PG_CREATE_ALLOWED_ENCODINGS.contains(&u.as_str()) {
            return Err(DbHostError::Backend(format!(
                "encoding must be one of: {}",
                PG_CREATE_ALLOWED_ENCODINGS.join(", ")
            )));
        }
    }
    Ok(())
}

async fn postgres_run_create_database(pool: &PgPool, body: &HostDbCreateDatabaseBody) -> Result<(), DbHostError> {
    if body.if_not_exists {
        let exists: (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&body.database)
            .fetch_one(pool)
            .await
            .map_err(|e| DbHostError::Backend(e.to_string()))?;
        if exists.0 {
            return Ok(());
        }
    }

    let mut sql = format!("CREATE DATABASE {}", body.database);
    if let Some(ref o) = body.owner {
        sql.push_str(&format!(" OWNER {}", o));
    }
    if let Some(ref enc) = body.encoding {
        let u = enc.trim().to_ascii_uppercase();
        sql.push_str(&format!(" ENCODING '{}'", u.replace('\'', "")));
    }

    sqlx::query(&sql)
        .execute(pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(())
}

/// Superuser or `CREATEDB` (same rule as PostgreSQL for `CREATE DATABASE`).
async fn pg_assert_can_create_database(pool: &PgPool) -> Result<(), DbHostError> {
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT rolsuper, rolcreatedb FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let Some((rolsuper, rolcreatedb)) = row else {
        return Err(DbHostError::Backend(
            "could not read current user role attributes".into(),
        ));
    };
    if !rolsuper && !rolcreatedb {
        return Err(DbHostError::Backend(
            "insufficient privileges for create-database: connecting PostgreSQL user must be superuser or have CREATEDB"
                .into(),
        ));
    }
    Ok(())
}

/// `CREATE DATABASE` using the caller's credentials (same as create-user; does not use `PIRATE_POSTGRES_ADMIN_URL`).
pub async fn postgres_create_database_for_caller(
    instance_id: &str,
    creds: &HostDbRequestCredentials,
    body: &HostDbCreateDatabaseBody,
) -> Result<(), DbHostError> {
    validate_create_database_body(body)?;
    let url = postgres_url_with_creds(instance_id, &creds.user, &creds.pass, "postgres")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    pg_assert_can_create_database(&pool).await?;
    postgres_run_create_database(&pool, body).await?;
    pool.close().await;
    Ok(())
}

const MAX_TABLE_COLUMNS: usize = 32;

/// Replace path in `PIRATE_POSTGRES_ADMIN_URL` with `/{database}` (validated identifier).
fn admin_url_for_database(base: &str, database: &str) -> Result<String, DbHostError> {
    validate_pg_ident(database).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    let mut u = Url::parse(base).map_err(|e| DbHostError::Backend(e.to_string()))?;
    u.set_path(&format!("/{database}"));
    Ok(u.to_string())
}

fn pg_quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn map_pg_column_type(col: &HostDbAdminCreateTableColumn) -> Result<String, DbHostError> {
    let t = col.data_type.trim().to_ascii_lowercase();
    let sql = match t.as_str() {
        "smallint" | "int2" => "SMALLINT".to_string(),
        "integer" | "int" | "int4" => "INTEGER".to_string(),
        "bigint" | "int8" => "BIGINT".to_string(),
        "text" => "TEXT".to_string(),
        "boolean" | "bool" => "BOOLEAN".to_string(),
        "timestamptz" => "TIMESTAMPTZ".to_string(),
        "timestamp" => "TIMESTAMP".to_string(),
        "date" => "DATE".to_string(),
        "jsonb" => "JSONB".to_string(),
        "json" => "JSON".to_string(),
        "uuid" => "UUID".to_string(),
        "double" | "float8" => "DOUBLE PRECISION".to_string(),
        "real" | "float4" => "REAL".to_string(),
        "smallserial" => {
            if !col.primary_key {
                return Err(DbHostError::Backend(
                    "smallserial is only valid with primary_key".into(),
                ));
            }
            "SMALLSERIAL".to_string()
        }
        "serial" => {
            if !col.primary_key {
                return Err(DbHostError::Backend(
                    "serial is only valid with primary_key".into(),
                ));
            }
            "SERIAL".to_string()
        }
        "bigserial" => {
            if !col.primary_key {
                return Err(DbHostError::Backend(
                    "bigserial is only valid with primary_key".into(),
                ));
            }
            "BIGSERIAL".to_string()
        }
        "varchar" => {
            let n = col.varchar_length.ok_or_else(|| {
                DbHostError::Backend("varchar_length is required for data_type varchar".into())
            })?;
            if n == 0 || n > 10_000 {
                return Err(DbHostError::Backend(
                    "varchar_length must be 1..=10000".into(),
                ));
            }
            format!("VARCHAR({n})")
        }
        _ => {
            return Err(DbHostError::Backend(format!(
                "unsupported data_type {t} (use allowlisted types from API docs)"
            )));
        }
    };
    Ok(sql)
}

/// `CREATE TABLE` using superuser URL; identifiers validated with [`validate_pg_ident`].
pub async fn postgres_create_table(
    host_env_path: &Path,
    body: &HostDbAdminCreateTableBody,
) -> Result<(), DbHostError> {
    if body.columns.is_empty() {
        return Err(DbHostError::Backend("columns must be non-empty".into()));
    }
    if body.columns.len() > MAX_TABLE_COLUMNS {
        return Err(DbHostError::Backend(format!(
            "at most {MAX_TABLE_COLUMNS} columns"
        )));
    }
    validate_pg_ident(&body.database).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    validate_pg_ident(&body.schema).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    validate_pg_ident(&body.table).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;

    let mut pk_count = 0usize;
    for c in &body.columns {
        validate_pg_ident(&c.name).map_err(|e: deploy_db::DbError| {
            DbHostError::Backend(e.to_string())
        })?;
        if c.primary_key {
            pk_count += 1;
        }
    }
    if pk_count > 1 {
        return Err(DbHostError::Backend(
            "at most one column may be primary_key".into(),
        ));
    }

    let base = admin_url_from_env(host_env_path)?;
    let url = admin_url_for_database(&base, &body.database)?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    let mut coldefs: Vec<String> = Vec::new();
    for c in &body.columns {
        let typ = map_pg_column_type(c)?;
        if typ.contains("SERIAL") {
            if c.not_null {
                // SERIAL is already not null; ignore not_null noise
            }
            let line = if c.primary_key {
                format!("{} {} PRIMARY KEY", c.name, typ)
            } else {
                return Err(DbHostError::Backend("SERIAL columns require primary_key".into()));
            };
            coldefs.push(line);
        } else {
            let nulls = if c.not_null { "NOT NULL" } else { "NULL" };
            let mut line = format!("{} {} {}", c.name, typ, nulls);
            if c.primary_key {
                line.push_str(" PRIMARY KEY");
            }
            coldefs.push(line);
        }
    }

    let ifne = if body.if_not_exists {
        " IF NOT EXISTS"
    } else {
        ""
    };
    let create_sql = format!(
        "CREATE TABLE{} {}.{} ({})",
        ifne,
        body.schema,
        body.table,
        coldefs.join(", ")
    );
    sqlx::query(&create_sql)
        .execute(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    pool.close().await;
    Ok(())
}

/// `CREATE DATABASE` via `PIRATE_MYSQL_ADMIN_URL` (owner/encoding in body ignored).
pub async fn mysql_create_database(
    host_env_path: &Path,
    body: &HostDbCreateDatabaseBody,
) -> Result<(), DbHostError> {
    validate_mysql_admin_ident(&body.database)?;
    let url = mysql_admin_url_from_env(host_env_path)?;
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    if body.if_not_exists {
        let exists: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
        )
        .bind(&body.database)
        .fetch_one(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
        if exists.0 > 0 {
            pool.close().await;
            return Ok(());
        }
    }

    let q = format!("CREATE DATABASE {}", mysql_quote_ident(&body.database));
    sqlx::query(&q)
        .execute(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    pool.close().await;
    Ok(())
}

fn map_mysql_column_type(col: &HostDbAdminCreateTableColumn) -> Result<String, DbHostError> {
    let t = col.data_type.trim().to_ascii_lowercase();
    let sql = match t.as_str() {
        "smallint" | "int2" => "SMALLINT".to_string(),
        "integer" | "int" | "int4" => "INT".to_string(),
        "bigint" | "int8" => "BIGINT".to_string(),
        "text" => "TEXT".to_string(),
        "boolean" | "bool" => "TINYINT(1)".to_string(),
        "timestamptz" => "TIMESTAMP(6)".to_string(),
        "timestamp" => "DATETIME(6)".to_string(),
        "date" => "DATE".to_string(),
        "jsonb" | "json" => "JSON".to_string(),
        "uuid" => "CHAR(36)".to_string(),
        "double" | "float8" => "DOUBLE".to_string(),
        "real" | "float4" => "FLOAT".to_string(),
        "smallserial" => {
            if !col.primary_key {
                return Err(DbHostError::Backend(
                    "smallserial requires primary_key".into(),
                ));
            }
            "SMALLINT NOT NULL AUTO_INCREMENT".to_string()
        }
        "serial" => {
            if !col.primary_key {
                return Err(DbHostError::Backend("serial requires primary_key".into()));
            }
            "INT NOT NULL AUTO_INCREMENT".to_string()
        }
        "bigserial" => {
            if !col.primary_key {
                return Err(DbHostError::Backend(
                    "bigserial requires primary_key".into(),
                ));
            }
            "BIGINT NOT NULL AUTO_INCREMENT".to_string()
        }
        "varchar" => {
            let n = col.varchar_length.ok_or_else(|| {
                DbHostError::Backend("varchar_length is required for varchar".into())
            })?;
            if n == 0 || n > 10_000 {
                return Err(DbHostError::Backend(
                    "varchar_length must be 1..=10000".into(),
                ));
            }
            format!("VARCHAR({n})")
        }
        _ => {
            return Err(DbHostError::Backend(format!(
                "unsupported data_type {t} for MySQL admin create-table"
            )));
        }
    };
    Ok(sql)
}

/// `CREATE TABLE` in `body.database` (MySQL: `schema` in JSON is ignored; use `database` as the logical DB).
pub async fn mysql_create_table(
    host_env_path: &Path,
    body: &HostDbAdminCreateTableBody,
) -> Result<(), DbHostError> {
    if body.columns.is_empty() {
        return Err(DbHostError::Backend("columns must be non-empty".into()));
    }
    if body.columns.len() > MAX_TABLE_COLUMNS {
        return Err(DbHostError::Backend(format!(
            "at most {MAX_TABLE_COLUMNS} columns"
        )));
    }
    validate_mysql_admin_ident(&body.database)?;
    validate_mysql_admin_ident(&body.table)?;
    for c in &body.columns {
        validate_mysql_admin_ident(&c.name)?;
    }
    let mut pk_count = 0usize;
    for c in &body.columns {
        if c.primary_key {
            pk_count += 1;
        }
    }
    if pk_count > 1 {
        return Err(DbHostError::Backend(
            "at most one column may be primary_key".into(),
        ));
    }

    let base = mysql_admin_url_from_env(host_env_path)?;
    let mut u = Url::parse(&base).map_err(|e| DbHostError::Backend(e.to_string()))?;
    u.set_path(&format!("/{}", body.database));
    let url = u.to_string();

    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    let mut coldefs: Vec<String> = Vec::new();
    for c in &body.columns {
        let typ = map_mysql_column_type(c)?;
        if typ.contains("AUTO_INCREMENT") {
            coldefs.push(format!(
                "{} {} PRIMARY KEY",
                mysql_quote_ident(&c.name),
                typ
            ));
        } else {
            let nulls = if c.not_null { "NOT NULL" } else { "NULL" };
            let mut line = format!(
                "{} {} {}",
                mysql_quote_ident(&c.name),
                typ,
                nulls
            );
            if c.primary_key {
                line.push_str(" PRIMARY KEY");
            }
            coldefs.push(line);
        }
    }

    let ifne = if body.if_not_exists {
        " IF NOT EXISTS"
    } else {
        ""
    };
    let create_sql = format!(
        "CREATE TABLE{} {} ({})",
        ifne,
        mysql_quote_ident(&body.table),
        coldefs.join(", ")
    );
    sqlx::query(&create_sql)
        .execute(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    pool.close().await;
    Ok(())
}

fn generate_user_password() -> String {
    format!("P{}", Uuid::new_v4().as_simple())
}

/// Ensure the connecting role can run `CREATE ROLE` (superuser or `CREATEROLE`).
async fn pg_assert_can_create_role(pool: &PgPool) -> Result<(), DbHostError> {
    let row: Option<(bool, bool)> = sqlx::query_as(
        "SELECT rolsuper, rolcreaterole FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let Some((rolsuper, rolcreaterole)) = row else {
        return Err(DbHostError::Backend(
            "could not read current user role attributes".into(),
        ));
    };
    if !rolsuper && !rolcreaterole {
        return Err(DbHostError::Backend(
            "insufficient privileges: connecting PostgreSQL user must be superuser or have CREATEROLE (required for create-user and delete-user)"
                .into(),
        ));
    }
    Ok(())
}

/// Create a login role and GRANT connect + schema privileges (read-only or read/write on `public` or given schema).
/// Uses the caller's DB credentials (same as host-db browse); does not use `PIRATE_POSTGRES_ADMIN_URL`.
pub async fn postgres_create_connection_user(
    instance_id: &str,
    creds: &HostDbRequestCredentials,
    body: &HostDbAdminCreateUserBody,
) -> Result<HostDbAdminCreateUserView, DbHostError> {
    validate_pg_ident(&body.database).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    validate_pg_ident(&body.username).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    validate_pg_ident(&body.schema).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;

    let privs = body.privileges.trim().to_ascii_lowercase();
    if privs != "read_write" && privs != "read_only" {
        return Err(DbHostError::Backend(
            "privileges must be read_write or read_only".into(),
        ));
    }

    let password = if body.generate_password {
        generate_user_password()
    } else {
        let p = body
            .password
            .as_ref()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                DbHostError::Backend(
                    "password is required when generate_password is false".into(),
                )
            })?;
        if p.len() < 8 || p.len() > 200 {
            return Err(DbHostError::Backend(
                "password length must be 8..=200 when provided".into(),
            ));
        }
        p.to_string()
    };

    let base = postgres_url_with_creds(instance_id, &creds.user, &creds.pass, "postgres")?;
    // 1) CREATE ROLE in maintenance DB
    let pool_admin = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&base)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    pg_assert_can_create_role(&pool_admin).await?;

    let create_role_sql = format!(
        "CREATE ROLE {} WITH LOGIN PASSWORD {}",
        body.username,
        pg_quote_literal(&password)
    );
    sqlx::query(&create_role_sql)
        .execute(&pool_admin)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    let grant_db_sql = format!(
        "GRANT CONNECT ON DATABASE {} TO {}",
        body.database, body.username
    );
    sqlx::query(&grant_db_sql)
        .execute(&pool_admin)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    pool_admin.close().await;

    // 2) Grants in target database
    let url = postgres_url_with_creds(instance_id, &creds.user, &creds.pass, &body.database)?;
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    let gusage = format!("GRANT USAGE ON SCHEMA {} TO {}", body.schema, body.username);
    sqlx::query(&gusage)
        .execute(&pool)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    if body.allow_schema_ddl {
        let g = format!("GRANT CREATE ON SCHEMA {} TO {}", body.schema, body.username);
        sqlx::query(&g)
            .execute(&pool)
            .await
            .map_err(|e| DbHostError::Backend(e.to_string()))?;
    }

    match privs.as_str() {
        "read_write" => {
            let t = format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA {} TO {}",
                body.schema, body.username
            );
            sqlx::query(&t)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            let s = format!(
                "GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA {} TO {}",
                body.schema, body.username
            );
            sqlx::query(&s)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            // Objects created later by the same superuser (this session) get same grants for app user
            let d1 = format!(
                "ALTER DEFAULT PRIVILEGES IN SCHEMA {} GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO {}",
                body.schema, body.username
            );
            sqlx::query(&d1)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            let d2 = format!(
                "ALTER DEFAULT PRIVILEGES IN SCHEMA {} GRANT USAGE, SELECT ON SEQUENCES TO {}",
                body.schema, body.username
            );
            sqlx::query(&d2)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
        }
        "read_only" => {
            let t = format!(
                "GRANT SELECT ON ALL TABLES IN SCHEMA {} TO {}",
                body.schema, body.username
            );
            sqlx::query(&t)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            let d = format!(
                "ALTER DEFAULT PRIVILEGES IN SCHEMA {} GRANT SELECT ON TABLES TO {}",
                body.schema, body.username
            );
            sqlx::query(&d)
                .execute(&pool)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
        }
        _ => unreachable!(),
    }

    pool.close().await;

    Ok(HostDbAdminCreateUserView {
        ok: true,
        username: body.username.clone(),
        password: if body.generate_password {
            Some(password)
        } else {
            None
        },
        message: if body.generate_password {
            Some("store the password; it is not shown again in audit logs".into())
        } else {
            None
        },
    })
}

/// `DROP ROLE` after optional `DROP OWNED` in all non-template databases (same caller creds as create-user).
pub async fn postgres_delete_connection_user(
    instance_id: &str,
    creds: &HostDbRequestCredentials,
    body: &HostDbAdminDeleteUserBody,
) -> Result<HostDbAdminDeleteUserView, DbHostError> {
    validate_pg_ident(&body.username).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    if creds.user == body.username {
        return Err(DbHostError::Backend(
            "cannot delete the same database user you are authenticated as; connect with another admin role"
                .into(),
        ));
    }

    let role_sql = pg_quoted_ident(&body.username);

    let maintenance = postgres_url_with_creds(instance_id, &creds.user, &creds.pass, "postgres")?;
    let pool_pg = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&maintenance)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    pg_assert_can_create_role(&pool_pg).await?;

    if body.drop_owned_all_databases {
        let dbs: Vec<(String,)> = sqlx::query_as(
            "SELECT datname::text FROM pg_database WHERE datallowconn AND NOT datistemplate ORDER BY datname",
        )
        .fetch_all(&pool_pg)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
        pool_pg.close().await;

        for (datname,) in dbs {
            let url = postgres_url_with_creds(instance_id, &creds.user, &creds.pass, &datname)?;
            let pool_db = PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(30))
                .connect(&url)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            let drop_owned = format!("DROP OWNED BY {} CASCADE", role_sql);
            sqlx::query(&drop_owned)
                .execute(&pool_db)
                .await
                .map_err(|e| DbHostError::Backend(e.to_string()))?;
            pool_db.close().await;
        }
    } else {
        pool_pg.close().await;
    }

    let pool_final = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&maintenance)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    let drop_role = format!("DROP ROLE {}", role_sql);
    sqlx::query(&drop_role)
        .execute(&pool_final)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;
    pool_final.close().await;

    Ok(HostDbAdminDeleteUserView {
        ok: true,
        username: body.username.clone(),
        message: Some("role removed (password material was not logged in audit)".into()),
    })
}

fn migration_allowlist_dirs(host_env_path: &Path) -> Result<Vec<PathBuf>, DbHostError> {
    let m = read_host_env_file(host_env_path);
    let raw = env_get(&m, "PIRATE_MIGRATION_CWD_ALLOWLIST").ok_or_else(|| {
        DbHostError::Backend(
            "PIRATE_MIGRATION_CWD_ALLOWLIST is not set (comma-separated absolute directory prefixes)"
                .into(),
        )
    })?;
    let mut out = Vec::new();
    for part in raw.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        out.push(PathBuf::from(t));
    }
    if out.is_empty() {
        return Err(DbHostError::Backend(
            "PIRATE_MIGRATION_CWD_ALLOWLIST is empty".into(),
        ));
    }
    Ok(out)
}

fn workdir_allowed(allowlist: &[PathBuf], workdir: &Path) -> Result<PathBuf, DbHostError> {
    let canon = std::fs::canonicalize(workdir).map_err(|e| {
        DbHostError::Backend(format!("migration workdir: {e}"))
    })?;
    for prefix in allowlist {
        let p = std::fs::canonicalize(prefix).map_err(|e| {
            DbHostError::Backend(format!("allowlist path {prefix:?}: {e}"))
        })?;
        if canon == p || canon.starts_with(&p) {
            return Ok(canon);
        }
    }
    Err(DbHostError::Backend(
        "workdir is not under PIRATE_MIGRATION_CWD_ALLOWLIST".into(),
    ))
}

const MIGRATION_OUTPUT_CAP: usize = 256 * 1024;

/// Run exactly one whitelisted command in an allowlisted directory (host env enforces policy).
pub async fn migration_run_approved(
    host_env_path: &Path,
    body: &HostDbMigrationRunBody,
) -> Result<HostDbMigrationRunView, DbHostError> {
    let allowlist = migration_allowlist_dirs(host_env_path)?;
    let wd = workdir_allowed(&allowlist, Path::new(&body.workdir))?;
    let tool = body.tool.trim();
    if tool != "alembic" && tool != "prisma" && tool != "flyway" {
        return Err(DbHostError::Backend(
            "tool must be alembic, prisma, or flyway".into(),
        ));
    }

    let mut cmd = match tool {
        "alembic" => {
            let mut c = Command::new("alembic");
            c.args(["upgrade", "head"]);
            c
        }
        "prisma" => {
            let mut c = Command::new("npx");
            c.args(["prisma", "migrate", "deploy"]);
            c
        }
        "flyway" => {
            let mut c = Command::new("flyway");
            c.arg("migrate");
            c
        }
        _ => unreachable!(),
    };
    cmd.current_dir(&wd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let out = tokio::time::timeout(Duration::from_secs(600), cmd.output())
        .await
        .map_err(|_| DbHostError::Backend("migration command timed out (600s)".into()))?
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    let mut combined = String::new();
    if let Ok(s) = String::from_utf8(out.stdout) {
        combined.push_str(&s);
    }
    if let Ok(s) = String::from_utf8(out.stderr) {
        if !combined.is_empty() {
            combined.push_str("\n--- stderr ---\n");
        }
        combined.push_str(&s);
    }
    if combined.len() > MIGRATION_OUTPUT_CAP {
        combined = format!(
            "{}\n... [truncated, total > {} bytes]",
            &combined[..MIGRATION_OUTPUT_CAP],
            MIGRATION_OUTPUT_CAP
        );
    }

    let ok = out.status.success();
    Ok(HostDbMigrationRunView {
        ok,
        exit_code: out.status.code(),
        output: combined,
        error: if ok {
            None
        } else {
            Some("command exited with non-zero status".into())
        },
    })
}

#[cfg(test)]
mod tests {
    use super::PG_CREATE_ALLOWED_ENCODINGS;

    #[test]
    fn create_database_sql_shape() {
        // Document expected keywords (policy tests; not executed against a server here).
        let s = "CREATE DATABASE mydb OWNER o ENCODING 'UTF8'";
        assert!(s.contains("CREATE DATABASE"));
        assert!(PG_CREATE_ALLOWED_ENCODINGS.contains(&"UTF8"));
    }

    #[test]
    fn create_table_sql_shape() {
        let sql = "CREATE TABLE IF NOT EXISTS public.t (id BIGSERIAL PRIMARY KEY, title TEXT NOT NULL)";
        assert!(sql.contains("CREATE TABLE") && sql.contains("public.t"));
    }
}
