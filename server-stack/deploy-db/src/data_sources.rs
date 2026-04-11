//! Persisted data sources (SMB mounts + DB connections) for the control dashboard.

use crate::{DbError, DbStore};
use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DataSourceRow {
    pub id: Uuid,
    pub kind: String,
    pub label: String,
    pub smb_host: Option<String>,
    pub smb_share: Option<String>,
    pub smb_subpath: Option<String>,
    pub mount_point: Option<String>,
    pub credentials_path: Option<String>,
    pub mount_state: String,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_json: Option<JsonValue>,
}

impl DbStore {
    pub async fn data_sources_list_all(&self) -> Result<Vec<DataSourceRow>, DbError> {
        let rows = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, DataSourceRow>(
                    r#"
            SELECT id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
                   credentials_path, mount_state, last_error, created_at, config_json
            FROM data_sources
            ORDER BY created_at DESC
            "#,
                )
                .fetch_all(pool)
                .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, DataSourceRow>(
                    r#"
            SELECT id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
                   credentials_path, mount_state, last_error, created_at, config_json
            FROM data_sources
            ORDER BY created_at DESC
            "#,
                )
                .fetch_all(pool)
                .await?
            }
        };
        Ok(rows)
    }

    pub async fn data_sources_list_smb(&self) -> Result<Vec<DataSourceRow>, DbError> {
        let rows = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, DataSourceRow>(
                    r#"
            SELECT id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
                   credentials_path, mount_state, last_error, created_at, config_json
            FROM data_sources
            WHERE kind = 'smb'
            ORDER BY created_at DESC
            "#,
                )
                .fetch_all(pool)
                .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, DataSourceRow>(
                    r#"
            SELECT id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
                   credentials_path, mount_state, last_error, created_at, config_json
            FROM data_sources
            WHERE kind = 'smb'
            ORDER BY created_at DESC
            "#,
                )
                .fetch_all(pool)
                .await?
            }
        };
        Ok(rows)
    }

    pub async fn data_sources_get(&self, id: Uuid) -> Result<Option<DataSourceRow>, DbError> {
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, DataSourceRow>(
                    r#"
            SELECT id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
                   credentials_path, mount_state, last_error, created_at, config_json
            FROM data_sources WHERE id = $1
            "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, DataSourceRow>(
                    r#"
            SELECT id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
                   credentials_path, mount_state, last_error, created_at, config_json
            FROM data_sources WHERE id = $1
            "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
            }
        };
        Ok(row)
    }

    pub async fn data_sources_insert_smb(
        &self,
        id: Uuid,
        label: &str,
        smb_host: &str,
        smb_share: &str,
        smb_subpath: &str,
        mount_point: &str,
        credentials_path: &str,
        mount_state: &str,
        last_error: Option<&str>,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO data_sources (
              id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
              credentials_path, mount_state, last_error, config_json
            )
            VALUES ($1, 'smb', $2, $3, $4, $5, $6, $7, $8, $9, NULL)
            "#,
                )
                .bind(id)
                .bind(label)
                .bind(smb_host)
                .bind(smb_share)
                .bind(smb_subpath)
                .bind(mount_point)
                .bind(credentials_path)
                .bind(mount_state)
                .bind(last_error)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO data_sources (
              id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
              credentials_path, mount_state, last_error, config_json
            )
            VALUES ($1, 'smb', $2, $3, $4, $5, $6, $7, $8, $9, NULL)
            "#,
                )
                .bind(id)
                .bind(label)
                .bind(smb_host)
                .bind(smb_share)
                .bind(smb_subpath)
                .bind(mount_point)
                .bind(credentials_path)
                .bind(mount_state)
                .bind(last_error)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn data_sources_insert_connection(
        &self,
        id: Uuid,
        kind: &str,
        label: &str,
        config_json: &JsonValue,
        credentials_path: Option<&str>,
        mount_state: &str,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO data_sources (
              id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
              credentials_path, mount_state, last_error, config_json
            )
            VALUES ($1, $2, $3, NULL, NULL, NULL, NULL, $4, $5, NULL, $6)
            "#,
                )
                .bind(id)
                .bind(kind)
                .bind(label)
                .bind(credentials_path)
                .bind(mount_state)
                .bind(config_json)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO data_sources (
              id, kind, label, smb_host, smb_share, smb_subpath, mount_point,
              credentials_path, mount_state, last_error, config_json
            )
            VALUES ($1, $2, $3, NULL, NULL, NULL, NULL, $4, $5, NULL, $6)
            "#,
                )
                .bind(id)
                .bind(kind)
                .bind(label)
                .bind(credentials_path)
                .bind(mount_state)
                .bind(config_json)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn data_sources_update_mount_state(
        &self,
        id: Uuid,
        mount_state: &str,
        last_error: Option<&str>,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            UPDATE data_sources SET mount_state = $2, last_error = $3 WHERE id = $1
            "#,
                )
                .bind(id)
                .bind(mount_state)
                .bind(last_error)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            UPDATE data_sources SET mount_state = $2, last_error = $3 WHERE id = $1
            "#,
                )
                .bind(id)
                .bind(mount_state)
                .bind(last_error)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn data_sources_delete(&self, id: Uuid) -> Result<u64, DbError> {
        match self {
            Self::Postgres(pool) => {
                let r = sqlx::query("DELETE FROM data_sources WHERE id = $1")
                    .bind(id)
                    .execute(pool)
                    .await?;
                Ok(r.rows_affected())
            }
            Self::Sqlite(pool) => {
                let r = sqlx::query("DELETE FROM data_sources WHERE id = $1")
                    .bind(id)
                    .execute(pool)
                    .await?;
                Ok(r.rows_affected())
            }
        }
    }
}
