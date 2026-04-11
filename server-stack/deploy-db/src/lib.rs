//! PostgreSQL pool, migrations, and helpers for deploy events + snapshot.

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub struct DbStore {
    pool: PgPool,
}

impl DbStore {
    pub async fn connect(database_url: &str) -> Result<Self, DbError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply schema migrations. Call this from **one** process only (typically `deploy-server`);
    /// other binaries should use [`Self::connect`] against an already-migrated database.
    pub async fn migrate(&self) -> Result<(), DbError> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn record_event(
        &self,
        project_id: &str,
        kind: &str,
        version: &str,
        state_snapshot: Option<&str>,
    ) -> Result<(), DbError> {
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_snapshot(
        &self,
        project_id: &str,
        current_version: &str,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO project_snapshots (project_id, current_version, state, last_error, updated_at)
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (project_id) DO UPDATE SET
              current_version = EXCLUDED.current_version,
              state = EXCLUDED.state,
              last_error = EXCLUDED.last_error,
              updated_at = NOW()
            "#,
        )
        .bind(project_id)
        .bind(current_version)
        .bind(state)
        .bind(last_error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_snapshot(&self, project_id: &str) -> Result<Option<SnapshotRow>, DbError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            r#"
            SELECT current_version, state, last_error, updated_at
            FROM project_snapshots WHERE project_id = $1
            "#,
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// When `project_id` is `None`, returns events for all projects (newest first).
    pub async fn fetch_history(
        &self,
        limit: i64,
        project_id: Option<&str>,
    ) -> Result<Vec<DeployEventRow>, DbError> {
        let rows = if let Some(pid) = project_id {
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
            .fetch_all(&self.pool)
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
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows)
    }

    pub async fn find_dashboard_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<DashboardUserRow>, DbError> {
        let row = sqlx::query_as::<_, DashboardUserRow>(
            r#"
            SELECT id, username, password_hash, created_at
            FROM dashboard_users WHERE username = $1
            "#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Insert new user or update password hash when `username` already exists.
    pub async fn upsert_dashboard_user(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO dashboard_users (username, password_hash)
            VALUES ($1, $2)
            ON CONFLICT (username) DO UPDATE SET password_hash = EXCLUDED.password_hash
            "#,
        )
        .bind(username)
        .bind(password_hash)
        .execute(&self.pool)
        .await?;
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
