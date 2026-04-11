//! Metadata store (PostgreSQL or SQLite) and PostgreSQL-only explorer helpers.

mod data_sources;
mod explorer;

pub use data_sources::DataSourceRow;
pub use explorer::{
    explorer_columns, explorer_foreign_keys, explorer_schemas, explorer_table_preview,
    explorer_tables, fetch_postgres_server_info, validate_pg_ident, ForeignKeyRow, PostgresServerInfoRow,
    SchemaRow, TableColumnRow, TablePreview, TableSummaryRow,
};
pub use sqlx::postgres::PgPool;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
}

/// Deploy metadata: events, snapshots, dashboard users, data sources.
pub enum DbStore {
    Postgres(PgPool),
    Sqlite(SqlitePool),
}

impl DbStore {
    /// Connect using `postgresql://…` or `sqlite:…` (including `sqlite::memory:`).
    pub async fn connect(database_url: &str) -> Result<Self, DbError> {
        let u = database_url.trim();
        if u.starts_with("sqlite:") {
            let pool = SqlitePoolOptions::new()
                .max_connections(5)
                .connect(u)
                .await?;
            Ok(Self::Sqlite(pool))
        } else {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(u)
                .await?;
            Ok(Self::Postgres(pool))
        }
    }

    #[must_use]
    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }

    /// When metadata is stored in PostgreSQL, the same pool may be used for the schema explorer.
    #[must_use]
    pub fn pg_pool(&self) -> Option<&PgPool> {
        match self {
            Self::Postgres(p) => Some(p),
            Self::Sqlite(_) => None,
        }
    }

    /// Apply schema migrations. Call from **one** process only (typically `deploy-server`).
    pub async fn migrate(&self) -> Result<(), DbError> {
        match self {
            Self::Postgres(p) => {
                sqlx::migrate!("./migrations").run(p).await?;
            }
            Self::Sqlite(p) => {
                sqlx::migrate!("./migrations_sqlite").run(p).await?;
            }
        }
        Ok(())
    }

    pub async fn record_event(
        &self,
        project_id: &str,
        kind: &str,
        version: &str,
        state_snapshot: Option<&str>,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO deploy_events (project_id, kind, version, state_snapshot)
            VALUES ($1, $2, $3, $4)
            "#,
                )
                .bind(project_id)
                .bind(kind)
                .bind(version)
                .bind(state_snapshot)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO deploy_events (project_id, kind, version, state_snapshot)
            VALUES ($1, $2, $3, $4)
            "#,
                )
                .bind(project_id)
                .bind(kind)
                .bind(version)
                .bind(state_snapshot)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn upsert_snapshot(
        &self,
        project_id: &str,
        current_version: &str,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), DbError> {
        let now = Utc::now();
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO project_snapshots (project_id, current_version, state, last_error, updated_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (project_id) DO UPDATE SET
              current_version = EXCLUDED.current_version,
              state = EXCLUDED.state,
              last_error = EXCLUDED.last_error,
              updated_at = EXCLUDED.updated_at
            "#,
                )
                .bind(project_id)
                .bind(current_version)
                .bind(state)
                .bind(last_error)
                .bind(now)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO project_snapshots (project_id, current_version, state, last_error, updated_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (project_id) DO UPDATE SET
              current_version = excluded.current_version,
              state = excluded.state,
              last_error = excluded.last_error,
              updated_at = excluded.updated_at
            "#,
                )
                .bind(project_id)
                .bind(current_version)
                .bind(state)
                .bind(last_error)
                .bind(now)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn get_snapshot(&self, project_id: &str) -> Result<Option<SnapshotRow>, DbError> {
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, SnapshotRow>(
                    r#"
            SELECT current_version, state, last_error, updated_at
            FROM project_snapshots WHERE project_id = $1
            "#,
                )
                .bind(project_id)
                .fetch_optional(pool)
                .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, SnapshotRow>(
                    r#"
            SELECT current_version, state, last_error, updated_at
            FROM project_snapshots WHERE project_id = $1
            "#,
                )
                .bind(project_id)
                .fetch_optional(pool)
                .await?
            }
        };
        Ok(row)
    }

    /// When `project_id` is `None`, returns events for all projects (newest first).
    pub async fn fetch_history(
        &self,
        limit: i64,
        project_id: Option<&str>,
    ) -> Result<Vec<DeployEventRow>, DbError> {
        let rows = match self {
            Self::Postgres(pool) => {
                if let Some(pid) = project_id {
                    sqlx::query_as::<_, DeployEventRow>(
                        r#"
                SELECT id, kind, version, created_at, state_snapshot, project_id
                FROM deploy_events
                WHERE project_id = $1
                ORDER BY id DESC
                LIMIT $2
                "#,
                    )
                    .bind(pid)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query_as::<_, DeployEventRow>(
                        r#"
                SELECT id, kind, version, created_at, state_snapshot, project_id
                FROM deploy_events
                ORDER BY id DESC
                LIMIT $1
                "#,
                    )
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
            }
            Self::Sqlite(pool) => {
                if let Some(pid) = project_id {
                    sqlx::query_as::<_, DeployEventRow>(
                        r#"
                SELECT id, kind, version, created_at, state_snapshot, project_id
                FROM deploy_events
                WHERE project_id = $1
                ORDER BY id DESC
                LIMIT $2
                "#,
                    )
                    .bind(pid)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query_as::<_, DeployEventRow>(
                        r#"
                SELECT id, kind, version, created_at, state_snapshot, project_id
                FROM deploy_events
                ORDER BY id DESC
                LIMIT $1
                "#,
                    )
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
            }
        };
        Ok(rows)
    }

    pub async fn find_dashboard_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<DashboardUserRow>, DbError> {
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, DashboardUserRow>(
                    r#"
            SELECT id, username, password_hash, created_at
            FROM dashboard_users WHERE username = $1
            "#,
                )
                .bind(username)
                .fetch_optional(pool)
                .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, DashboardUserRow>(
                    r#"
            SELECT id, username, password_hash, created_at
            FROM dashboard_users WHERE username = $1
            "#,
                )
                .bind(username)
                .fetch_optional(pool)
                .await?
            }
        };
        Ok(row)
    }

    pub async fn upsert_dashboard_user(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO dashboard_users (username, password_hash)
            VALUES ($1, $2)
            ON CONFLICT (username) DO UPDATE SET password_hash = EXCLUDED.password_hash
            "#,
                )
                .bind(username)
                .bind(password_hash)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO dashboard_users (username, password_hash)
            VALUES ($1, $2)
            ON CONFLICT (username) DO UPDATE SET password_hash = excluded.password_hash
            "#,
                )
                .bind(username)
                .bind(password_hash)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DashboardUserRow {
    pub id: i32,
    pub username: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct SnapshotRow {
    pub current_version: String,
    pub state: String,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DeployEventRow {
    pub id: i64,
    pub kind: String,
    pub version: String,
    pub created_at: DateTime<Utc>,
    pub state_snapshot: Option<String>,
    pub project_id: String,
}
