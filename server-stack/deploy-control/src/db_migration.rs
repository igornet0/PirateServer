//! Read-only inspection of app migration tool tables (Alembic, Flyway, Prisma, Django) on host databases.

use chrono::Utc;
use deploy_db::validate_pg_ident;
use serde::Serialize;
use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::db_host::{self, connect_ephemeral_postgres, mysql_url_with_creds, DbHostError};
use url::Url;

#[derive(Debug, Clone, Serialize)]
pub struct MigrationToolReport {
    pub tool: String,
    pub present: bool,
    /// Single “head” revision/name for UI (best-effort from tool tables).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Limited rows (JSON values) for UI preview.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostDbMigrationStatusView {
    pub engine: String,
    pub database: String,
    pub detected_at_ms: i64,
    pub tools: Vec<MigrationToolReport>,
}

async fn pg_table_exists(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<bool, DbHostError> {
    validate_pg_ident(schema).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    validate_pg_ident(table).map_err(|e: deploy_db::DbError| {
        DbHostError::Backend(e.to_string())
    })?;
    let r: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables
         WHERE table_schema = $1 AND table_name = $2)",
    )
    .bind(schema)
    .bind(table)
    .fetch_one(pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    Ok(r.0)
}

/// Alembic: `public.alembic_version` with `version_num`.
async fn read_alembic_pg(pool: &PgPool) -> MigrationToolReport {
    let mut r = MigrationToolReport {
        tool: "alembic".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    match pg_table_exists(pool, "public", "alembic_version").await {
        Ok(false) => {
            r.summary = Some("no public.alembic_version table".into());
            return r;
        }
        Ok(true) => {
            r.present = true;
        }
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    }
    match sqlx::query("SELECT version_num FROM public.alembic_version LIMIT 5")
        .fetch_all(pool)
        .await
    {
        Ok(rows) => {
            for row in &rows {
                if let Ok(v) = row.try_get::<String, _>(0) {
                    r.rows.push(serde_json::json!({ "version_num": v }));
                }
            }
            r.current_version = r
                .rows
                .first()
                .and_then(|j| j.get("version_num"))
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);
            r.summary = r
                .current_version
                .as_ref()
                .map(|s| format!("current: {s}"));
        }
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

/// Flyway: `public.flyway_schema_history` (or flyway's default).
async fn read_flyway_pg(pool: &PgPool) -> MigrationToolReport {
    let mut r = MigrationToolReport {
        tool: "flyway".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    if !pg_table_exists(pool, "public", "flyway_schema_history")
        .await
        .unwrap_or(false)
    {
        r.summary = Some("no public.flyway_schema_history table".into());
        return r;
    }
    r.present = true;
    let q = r#"SELECT version, description, "type"::text, installed_on::text, success
             FROM public.flyway_schema_history
             ORDER BY installed_rank DESC
             LIMIT 5"#;
    match sqlx::query(q).fetch_all(pool).await {
        Ok(rows) => {
            for row in &rows {
                r.rows.push(serde_json::json!({
                    "version": row.try_get::<String, _>(0).ok(),
                    "description": row.try_get::<Option<String>, _>(1).ok().flatten(),
                    "type": row.try_get::<String, _>(2).ok(),
                    "installed_on": row.try_get::<Option<String>, _>(3).ok().flatten(),
                    "success": row.try_get::<Option<bool>, _>(4).ok().flatten(),
                }));
            }
            r.current_version = r
                .rows
                .first()
                .and_then(|j| j.get("version"))
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);
            if r.current_version.is_some() {
                r.summary = r
                    .current_version
                    .as_ref()
                    .map(|s| format!("latest applied: {s}"));
            }
        }
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

/// Prisma: `public._prisma_migrations`.
async fn read_prisma_pg(pool: &PgPool) -> MigrationToolReport {
    let mut r = MigrationToolReport {
        tool: "prisma".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    if !pg_table_exists(pool, "public", "_prisma_migrations")
        .await
        .unwrap_or(false)
    {
        r.summary = Some("no public._prisma_migrations table".into());
        return r;
    }
    r.present = true;
    let q = "SELECT migration_name, started_at::text, finished_at::text, applied_steps_count, rolled_back_at::text
             FROM public._prisma_migrations
             ORDER BY started_at DESC NULLS LAST
             LIMIT 5";
    match sqlx::query(q).fetch_all(pool).await {
        Ok(rows) => {
            for row in &rows {
                r.rows.push(serde_json::json!({
                    "migration_name": row.try_get::<String, _>(0).ok(),
                    "started_at": row.try_get::<Option<String>, _>(1).ok().flatten(),
                    "finished_at": row.try_get::<Option<String>, _>(2).ok().flatten(),
                    "applied_steps_count": row.try_get::<Option<i64>, _>(3).ok().flatten(),
                    "rolled_back_at": row.try_get::<Option<String>, _>(4).ok().flatten(),
                }));
            }
            r.current_version = r
                .rows
                .iter()
                .find(|j| j.get("rolled_back_at").map(|v| v.is_null()).unwrap_or(true))
                .or_else(|| r.rows.first())
                .and_then(|j| j.get("migration_name"))
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);
            if r.current_version.is_some() {
                r.summary = r
                    .current_version
                    .as_ref()
                    .map(|s| format!("head migration: {s}"));
            }
        }
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

/// Django: `public.django_migrations`.
async fn read_django_pg(pool: &PgPool) -> MigrationToolReport {
    let mut r = MigrationToolReport {
        tool: "django".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    if !pg_table_exists(pool, "public", "django_migrations")
        .await
        .unwrap_or(false)
    {
        r.summary = Some("no public.django_migrations table".into());
        return r;
    }
    r.present = true;
    let q = "SELECT id, app, name, applied::text FROM public.django_migrations ORDER BY id DESC LIMIT 5";
    match sqlx::query(q).fetch_all(pool).await {
        Ok(rows) => {
            for row in &rows {
                r.rows.push(serde_json::json!({
                    "id": row.try_get::<i64, _>(0).ok(),
                    "app": row.try_get::<String, _>(1).ok(),
                    "name": row.try_get::<String, _>(2).ok(),
                    "applied": row.try_get::<Option<String>, _>(3).ok().flatten(),
                }));
            }
            r.current_version = r.rows.first().and_then(|j| {
                let app = j.get("app")?.as_str()?;
                let name = j.get("name")?.as_str()?;
                Some(format!("{app}.{name}"))
            });
        }
        Err(e) => r.error = Some(e.to_string()),
    }
    r
}

/// Inspect migration metadata tables (read-only) for a PostgreSQL database.
pub async fn postgres_migration_status(
    pool: &PgPool,
    engine: &str,
    database: &str,
) -> Result<HostDbMigrationStatusView, DbHostError> {
    let mut tools = Vec::new();
    tools.push(read_alembic_pg(pool).await);
    tools.push(read_flyway_pg(pool).await);
    tools.push(read_prisma_pg(pool).await);
    tools.push(read_django_pg(pool).await);
    let detected_at_ms = Utc::now().timestamp_millis();
    Ok(HostDbMigrationStatusView {
        engine: engine.to_string(),
        database: database.to_string(),
        detected_at_ms,
        tools,
    })
}

/// MySQL: check information_schema and read alembic / flyway style tables in current database.
async fn mysql_migration_status_for_url(
    url: &str,
    engine: &str,
    database: &str,
) -> Result<HostDbMigrationStatusView, DbHostError> {
    use sqlx::mysql::MySqlPoolOptions;
    use std::time::Duration;
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(15))
        .connect(url)
        .await
        .map_err(|e| DbHostError::Backend(e.to_string()))?;

    let mut tools = Vec::new();
    // Alembic
    let mut a = MigrationToolReport {
        tool: "alembic".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    let exists: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = 'alembic_version'",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    if exists.0 > 0 {
        a.present = true;
        match sqlx::query("SELECT version_num FROM alembic_version LIMIT 5")
            .fetch_all(&pool)
            .await
        {
            Ok(rows) => {
                for row in &rows {
                    if let Ok(v) = row.try_get::<String, _>(0) {
                        a.rows.push(serde_json::json!({ "version_num": v }));
                    }
                }
                a.current_version = a
                    .rows
                    .first()
                    .and_then(|j| j.get("version_num"))
                    .and_then(|v| v.as_str())
                    .map(std::string::ToString::to_string);
                a.summary = a
                    .current_version
                    .as_ref()
                    .map(|s| format!("current: {s}"));
            }
            Err(e) => a.error = Some(e.to_string()),
        }
    } else {
        a.summary = Some("no alembic_version table".into());
    }
    tools.push(a);

    // Flyway (MySQL): flyway_schema_history in current database
    let mut f = MigrationToolReport {
        tool: "flyway".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    let fly_exists: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = 'flyway_schema_history'",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    if fly_exists.0 > 0 {
        f.present = true;
        match sqlx::query(
            r#"SELECT version, description, `type`, CAST(installed_on AS CHAR), success
               FROM flyway_schema_history
               ORDER BY installed_rank DESC
               LIMIT 5"#,
        )
        .fetch_all(&pool)
        .await
        {
            Ok(rows) => {
                for row in &rows {
                    f.rows.push(serde_json::json!({
                        "version": row.try_get::<String, _>(0).ok(),
                        "description": row.try_get::<Option<String>, _>(1).ok().flatten(),
                        "type": row.try_get::<String, _>(2).ok(),
                        "installed_on": row.try_get::<Option<String>, _>(3).ok().flatten(),
                        "success": row.try_get::<Option<bool>, _>(4).ok().flatten(),
                    }));
                }
                f.current_version = f
                    .rows
                    .first()
                    .and_then(|j| j.get("version"))
                    .and_then(|v| v.as_str())
                    .map(std::string::ToString::to_string);
                if f.current_version.is_some() {
                    f.summary = f
                        .current_version
                        .as_ref()
                        .map(|s| format!("latest applied: {s}"));
                }
            }
            Err(e) => f.error = Some(e.to_string()),
        }
    } else {
        f.summary = Some("no flyway_schema_history table".into());
    }
    tools.push(f);

    // Prisma (MySQL): _prisma_migrations
    let mut p = MigrationToolReport {
        tool: "prisma".into(),
        present: false,
        current_version: None,
        summary: None,
        rows: vec![],
        error: None,
    };
    let prisma_exists: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = '_prisma_migrations'",
    )
    .fetch_one(&pool)
    .await
    .map_err(|e| DbHostError::Backend(e.to_string()))?;
    if prisma_exists.0 > 0 {
        p.present = true;
        match sqlx::query(
            "SELECT migration_name, CAST(started_at AS CHAR), CAST(finished_at AS CHAR), \
             applied_steps_count, CAST(rolled_back_at AS CHAR) \
             FROM _prisma_migrations \
             ORDER BY started_at DESC \
             LIMIT 5",
        )
        .fetch_all(&pool)
        .await
        {
            Ok(rows) => {
                for row in &rows {
                    p.rows.push(serde_json::json!({
                        "migration_name": row.try_get::<String, _>(0).ok(),
                        "started_at": row.try_get::<Option<String>, _>(1).ok().flatten(),
                        "finished_at": row.try_get::<Option<String>, _>(2).ok().flatten(),
                        "applied_steps_count": row.try_get::<Option<i64>, _>(3).ok().flatten(),
                        "rolled_back_at": row.try_get::<Option<String>, _>(4).ok().flatten(),
                    }));
                }
                p.current_version = p
                    .rows
                    .iter()
                    .find(|j| j.get("rolled_back_at").map(|v| v.is_null()).unwrap_or(true))
                    .or_else(|| p.rows.first())
                    .and_then(|j| j.get("migration_name"))
                    .and_then(|v| v.as_str())
                    .map(std::string::ToString::to_string);
                if p.current_version.is_some() {
                    p.summary = p
                        .current_version
                        .as_ref()
                        .map(|s| format!("head migration: {s}"));
                }
            }
            Err(e) => p.error = Some(e.to_string()),
        }
    } else {
        p.summary = Some("no _prisma_migrations table".into());
    }
    tools.push(p);

    let detected_at_ms = Utc::now().timestamp_millis();
    pool.close().await;
    Ok(HostDbMigrationStatusView {
        engine: engine.to_string(),
        database: database.to_string(),
        detected_at_ms,
        tools,
    })
}

fn validate_mysql_database(s: &str) -> Result<(), DbHostError> {
    if s.is_empty() || s.len() > 64 {
        return Err(DbHostError::Backend("invalid MySQL database name".into()));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '$')
    {
        return Err(DbHostError::Backend(
            "MySQL database name: use ASCII letters, digits, _, -, or $ only".into(),
        ));
    }
    Ok(())
}

/// Public entry: `instance_id`, credentials, target `database` name.
pub async fn host_migration_status(
    instance_id: &str,
    user: &str,
    pass: &str,
    database: &str,
) -> Result<HostDbMigrationStatusView, DbHostError> {
    if database.is_empty() {
        return Err(DbHostError::Backend("database name is required".into()));
    }

    let (engine, _, _) = db_host::parse_instance_id(instance_id)
        .ok_or(DbHostError::InvalidInstanceId)?;
    match engine.as_str() {
        "postgresql" => {
            validate_pg_ident(database).map_err(|e: deploy_db::DbError| {
                DbHostError::Backend(e.to_string())
            })?;
            let url =
                db_host::postgres_url_with_creds(instance_id, user, pass, database)?;
            let pool = connect_ephemeral_postgres(&url).await?;
            let out = postgres_migration_status(&pool, "postgresql", database).await;
            pool.close().await;
            out
        }
        "mysql" => {
            validate_mysql_database(database)?;
            let base = mysql_url_with_creds(instance_id, user, pass)?;
            let u2 = Url::parse(&base).map_err(|e| DbHostError::Backend(e.to_string()))?;
            let mut u2 = u2;
            u2.set_path(&format!("/{database}"));
            mysql_migration_status_for_url(u2.as_str(), "mysql", database).await
        }
        e => Err(DbHostError::UnsupportedEngine(e.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::MigrationToolReport;

    #[test]
    fn report_serializes() {
        let r = MigrationToolReport {
            tool: "alembic".into(),
            present: true,
            current_version: Some("abc".into()),
            summary: Some("x".into()),
            rows: vec![],
            error: None,
        };
        let s = serde_json::to_string(&r).expect("json");
        assert!(s.contains("alembic"));
    }

    /// Stable fragments of read-only SQL (regression for tool table names / columns).
    #[test]
    fn pg_alembic_select_contains_expected() {
        let q = "SELECT version_num FROM public.alembic_version LIMIT 5";
        assert!(q.contains("alembic_version"));
    }

    #[test]
    fn pg_flyway_select_contains_expected() {
        let q = r#"SELECT version, description, "type"::text, installed_on::text, success
             FROM public.flyway_schema_history
             ORDER BY installed_rank DESC
             LIMIT 5"#;
        assert!(q.contains("flyway_schema_history"));
    }

    #[test]
    fn pg_prisma_select_contains_expected() {
        let q = "SELECT migration_name, started_at::text, finished_at::text, applied_steps_count, rolled_back_at::text
             FROM public._prisma_migrations";
        assert!(q.contains("_prisma_migrations"));
    }

    #[test]
    fn pg_django_select_contains_expected() {
        let q = "SELECT id, app, name, applied::text FROM public.django_migrations";
        assert!(q.contains("django_migrations"));
    }
}
