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
        kind: &str,
        version: &str,
        state_snapshot: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO deploy_events (kind, version, state_snapshot)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(kind)
        .bind(version)
        .bind(state_snapshot)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_snapshot(
        &self,
        current_version: &str,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO service_snapshot (id, current_version, state, last_error, updated_at)
            VALUES (1, $1, $2, $3, NOW())
            ON CONFLICT (id) DO UPDATE SET
              current_version = EXCLUDED.current_version,
              state = EXCLUDED.state,
              last_error = EXCLUDED.last_error,
              updated_at = NOW()
            "#,
        )
        .bind(current_version)
        .bind(state)
        .bind(last_error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_snapshot(&self) -> Result<Option<SnapshotRow>, DbError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            r#"
            SELECT current_version, state, last_error, updated_at
            FROM service_snapshot WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn fetch_history(&self, limit: i64) -> Result<Vec<DeployEventRow>, DbError> {
        let rows = sqlx::query_as::<_, DeployEventRow>(
            r#"
            SELECT id, kind, version, created_at, state_snapshot
            FROM deploy_events
            ORDER BY id DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
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
}
