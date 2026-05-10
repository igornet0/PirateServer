//! Persisted total byte counter for the Pirate file storage module.

use crate::{DbError, DbStore};

impl DbStore {
    /// Total bytes of user-visible file objects under the storage root (updated by control-api).
    pub async fn pirate_file_storage_get_used_bytes(&self) -> Result<u64, DbError> {
        let row: (i64,) = match self {
            Self::Postgres(pool) => sqlx::query_as("SELECT used_bytes::bigint FROM pirate_file_storage_stats WHERE id = 1")
                .fetch_one(pool)
                .await?,
            Self::Sqlite(pool) => sqlx::query_as("SELECT used_bytes FROM pirate_file_storage_stats WHERE id = 1")
                .fetch_one(pool)
                .await?,
        };
        Ok(row.0.max(0) as u64)
    }

    pub async fn pirate_file_storage_set_used_bytes(&self, used: u64) -> Result<(), DbError> {
        let u = used as i64;
        match self {
            Self::Postgres(pool) => {
                sqlx::query("UPDATE pirate_file_storage_stats SET used_bytes = $1 WHERE id = 1")
                    .bind(u)
                    .execute(pool)
                    .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query("UPDATE pirate_file_storage_stats SET used_bytes = $1 WHERE id = 1")
                    .bind(u)
                    .execute(pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Delta may be negative. Result is the new `used_bytes` (clamped at 0).
    pub async fn pirate_file_storage_add_used_delta(&self, delta: i64) -> Result<u64, DbError> {
        let new_val: (i64,) = match self {
            Self::Postgres(pool) => sqlx::query_as(
                r#"
                UPDATE pirate_file_storage_stats
                SET used_bytes = GREATEST(0, used_bytes::bigint + $1)
                WHERE id = 1
                RETURNING used_bytes
                "#,
            )
            .bind(delta)
            .fetch_one(pool)
            .await?,
            Self::Sqlite(pool) => sqlx::query_as(
                r#"
                UPDATE pirate_file_storage_stats
                SET used_bytes = MAX(0, used_bytes + $1)
                WHERE id = 1
                RETURNING used_bytes
                "#,
            )
            .bind(delta)
            .fetch_one(pool)
            .await?,
        };
        Ok(new_val.0.max(0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::DbStore;

    #[tokio::test]
    async fn delta_roundtrip_sqlite() {
        let db = DbStore::connect("sqlite::memory:").await.expect("connect");
        db.migrate().await.expect("migrate");
        assert_eq!(db.pirate_file_storage_get_used_bytes().await.unwrap(), 0);
        assert_eq!(db.pirate_file_storage_add_used_delta(100).await.unwrap(), 100);
        assert_eq!(db.pirate_file_storage_add_used_delta(-30).await.unwrap(), 70);
        assert_eq!(db.pirate_file_storage_add_used_delta(-200).await.unwrap(), 0);
    }
}
