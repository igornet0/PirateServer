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

    pub async fn insert_grpc_session_event(
        &self,
        kind: &str,
        client_pubkey_b64: Option<&str>,
        peer_ip: &str,
        grpc_method: &str,
        status: &str,
        detail: &str,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_session_events (kind, client_pubkey_b64, peer_ip, grpc_method, status, detail)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
                )
                .bind(kind)
                .bind(client_pubkey_b64)
                .bind(peer_ip)
                .bind(grpc_method)
                .bind(status)
                .bind(detail)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_session_events (kind, client_pubkey_b64, peer_ip, grpc_method, status, detail)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
                )
                .bind(kind)
                .bind(client_pubkey_b64)
                .bind(peer_ip)
                .bind(grpc_method)
                .bind(status)
                .bind(detail)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Latest row per non-empty `client_pubkey_b64` (by max id).
    pub async fn fetch_grpc_session_peer_last_activity(
        &self,
    ) -> Result<Vec<GrpcSessionPeerAggregateRow>, DbError> {
        let q = r#"
            SELECT e.client_pubkey_b64 AS client_pubkey_b64,
                   e.id AS last_id,
                   e.created_at AS last_created_at,
                   e.peer_ip AS last_peer_ip,
                   e.grpc_method AS last_grpc_method
            FROM grpc_session_events e
            INNER JOIN (
                SELECT client_pubkey_b64 AS pk, MAX(id) AS mid
                FROM grpc_session_events
                WHERE client_pubkey_b64 IS NOT NULL AND TRIM(client_pubkey_b64) != ''
                GROUP BY client_pubkey_b64
            ) t ON e.client_pubkey_b64 = t.pk AND e.id = t.mid
            ORDER BY e.client_pubkey_b64
        "#;
        let rows = match self {
            Self::Postgres(pool) => sqlx::query_as::<_, GrpcSessionPeerAggregateRow>(q)
                .fetch_all(pool)
                .await?,
            Self::Sqlite(pool) => sqlx::query_as::<_, GrpcSessionPeerAggregateRow>(q)
                .fetch_all(pool)
                .await?,
        };
        Ok(rows)
    }

    /// Newest first. `before_id <= 0` means start from the newest rows.
    pub async fn fetch_grpc_session_events_page(
        &self,
        before_id: i64,
        limit: i64,
    ) -> Result<Vec<GrpcSessionEventRow>, DbError> {
        let rows = if before_id <= 0 {
            match self {
                Self::Postgres(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                ORDER BY id DESC
                LIMIT $1
                "#,
                    )
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
                Self::Sqlite(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                ORDER BY id DESC
                LIMIT $1
                "#,
                    )
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
            }
        } else {
            match self {
                Self::Postgres(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                WHERE id < $1
                ORDER BY id DESC
                LIMIT $2
                "#,
                    )
                    .bind(before_id)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
                Self::Sqlite(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                WHERE id < $1
                ORDER BY id DESC
                LIMIT $2
                "#,
                    )
                    .bind(before_id)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
            }
        };
        Ok(rows)
    }

    /// Like [`Self::fetch_grpc_session_events_page`], but omits low-level TCP audit rows.
    pub async fn fetch_grpc_session_events_page_no_tcp(
        &self,
        before_id: i64,
        limit: i64,
    ) -> Result<Vec<GrpcSessionEventRow>, DbError> {
        let rows = if before_id <= 0 {
            match self {
                Self::Postgres(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                WHERE kind NOT IN ('tcp_open', 'tcp_close')
                ORDER BY id DESC
                LIMIT $1
                "#,
                    )
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
                Self::Sqlite(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                WHERE kind NOT IN ('tcp_open', 'tcp_close')
                ORDER BY id DESC
                LIMIT $1
                "#,
                    )
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
            }
        } else {
            match self {
                Self::Postgres(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                WHERE id < $1 AND kind NOT IN ('tcp_open', 'tcp_close')
                ORDER BY id DESC
                LIMIT $2
                "#,
                    )
                    .bind(before_id)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
                Self::Sqlite(pool) => {
                    sqlx::query_as::<_, GrpcSessionEventRow>(
                        r#"
                SELECT id, created_at, kind, client_pubkey_b64, peer_ip, grpc_method, status, detail
                FROM grpc_session_events
                WHERE id < $1 AND kind NOT IN ('tcp_open', 'tcp_close')
                ORDER BY id DESC
                LIMIT $2
                "#,
                    )
                    .bind(before_id)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?
                }
            }
        };
        Ok(rows)
    }

    pub async fn count_grpc_session_events_total(&self) -> Result<i64, DbError> {
        let n = match self {
            Self::Postgres(pool) => {
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM grpc_session_events")
                    .fetch_one(pool)
                    .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM grpc_session_events")
                    .fetch_one(pool)
                    .await?
            }
        };
        Ok(n)
    }

    pub async fn fetch_grpc_session_kind_counts(&self) -> Result<Vec<GrpcSessionKindCount>, DbError> {
        let q_pg = r#"
            SELECT kind, COUNT(*)::bigint AS event_count
            FROM grpc_session_events
            GROUP BY kind
            ORDER BY kind
        "#;
        let q_sl = r#"
            SELECT kind, COUNT(*) AS event_count
            FROM grpc_session_events
            GROUP BY kind
            ORDER BY kind
        "#;
        let rows = match self {
            Self::Postgres(pool) => sqlx::query_as::<_, GrpcSessionKindCount>(q_pg)
                .fetch_all(pool)
                .await?,
            Self::Sqlite(pool) => sqlx::query_as::<_, GrpcSessionKindCount>(q_sl)
                .fetch_all(pool)
                .await?,
        };
        Ok(rows)
    }

    pub async fn upsert_grpc_peer_profile(
        &self,
        client_pubkey_b64: &str,
        connection_kind: i16,
        agent_version: &str,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_peer_profile (client_pubkey_b64, connection_kind, agent_version, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (client_pubkey_b64) DO UPDATE SET
              connection_kind = EXCLUDED.connection_kind,
              agent_version = EXCLUDED.agent_version,
              updated_at = NOW()
            "#,
                )
                .bind(client_pubkey_b64)
                .bind(connection_kind)
                .bind(agent_version)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_peer_profile (client_pubkey_b64, connection_kind, agent_version, updated_at)
            VALUES ($1, $2, $3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            ON CONFLICT (client_pubkey_b64) DO UPDATE SET
              connection_kind = excluded.connection_kind,
              agent_version = excluded.agent_version,
              updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
                )
                .bind(client_pubkey_b64)
                .bind(connection_kind)
                .bind(agent_version)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn upsert_grpc_peer_resource_snapshot(
        &self,
        client_pubkey_b64: &str,
        cpu_percent: Option<f64>,
        ram_percent: Option<f64>,
        gpu_percent: Option<f64>,
        ram_used_bytes: Option<i64>,
        storage_used_bytes: Option<i64>,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_peer_resource_snapshot (
              client_pubkey_b64, cpu_percent, ram_percent, gpu_percent,
              ram_used_bytes, storage_used_bytes, reported_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, NOW())
            ON CONFLICT (client_pubkey_b64) DO UPDATE SET
              cpu_percent = EXCLUDED.cpu_percent,
              ram_percent = EXCLUDED.ram_percent,
              gpu_percent = EXCLUDED.gpu_percent,
              ram_used_bytes = EXCLUDED.ram_used_bytes,
              storage_used_bytes = EXCLUDED.storage_used_bytes,
              reported_at = NOW()
            "#,
                )
                .bind(client_pubkey_b64)
                .bind(cpu_percent)
                .bind(ram_percent)
                .bind(gpu_percent)
                .bind(ram_used_bytes)
                .bind(storage_used_bytes)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_peer_resource_snapshot (
              client_pubkey_b64, cpu_percent, ram_percent, gpu_percent,
              ram_used_bytes, storage_used_bytes, reported_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            ON CONFLICT (client_pubkey_b64) DO UPDATE SET
              cpu_percent = excluded.cpu_percent,
              ram_percent = excluded.ram_percent,
              gpu_percent = excluded.gpu_percent,
              ram_used_bytes = excluded.ram_used_bytes,
              storage_used_bytes = excluded.storage_used_bytes,
              reported_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
                )
                .bind(client_pubkey_b64)
                .bind(cpu_percent)
                .bind(ram_percent)
                .bind(gpu_percent)
                .bind(ram_used_bytes)
                .bind(storage_used_bytes)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Add bytes to the hourly bucket for this peer (upsert increment).
    pub async fn add_grpc_proxy_traffic_hourly(
        &self,
        client_pubkey_b64: &str,
        hour_start: DateTime<Utc>,
        bytes_in: u64,
        bytes_out: u64,
    ) -> Result<(), DbError> {
        let hour_sqlite = hour_start.to_rfc3339();
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_proxy_traffic_hourly (client_pubkey_b64, hour_start, bytes_in, bytes_out)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (client_pubkey_b64, hour_start) DO UPDATE SET
              bytes_in = grpc_proxy_traffic_hourly.bytes_in + EXCLUDED.bytes_in,
              bytes_out = grpc_proxy_traffic_hourly.bytes_out + EXCLUDED.bytes_out
            "#,
                )
                .bind(client_pubkey_b64)
                .bind(hour_start)
                .bind(bytes_in as i64)
                .bind(bytes_out as i64)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_proxy_traffic_hourly (client_pubkey_b64, hour_start, bytes_in, bytes_out)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (client_pubkey_b64, hour_start) DO UPDATE SET
              bytes_in = grpc_proxy_traffic_hourly.bytes_in + excluded.bytes_in,
              bytes_out = grpc_proxy_traffic_hourly.bytes_out + excluded.bytes_out
            "#,
                )
                .bind(client_pubkey_b64)
                .bind(hour_sqlite)
                .bind(bytes_in as i64)
                .bind(bytes_out as i64)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn sum_grpc_proxy_traffic_totals(
        &self,
        client_pubkey_b64: &str,
    ) -> Result<(u64, u64), DbError> {
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, (i64, i64)>(
                    r#"
            SELECT COALESCE(SUM(bytes_in), 0)::bigint, COALESCE(SUM(bytes_out), 0)::bigint
            FROM grpc_proxy_traffic_hourly
            WHERE client_pubkey_b64 = $1
            "#,
                )
                .bind(client_pubkey_b64)
                .fetch_one(pool)
                .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, (i64, i64)>(
                    r#"
            SELECT COALESCE(SUM(bytes_in), 0), COALESCE(SUM(bytes_out), 0)
            FROM grpc_proxy_traffic_hourly
            WHERE client_pubkey_b64 = $1
            "#,
                )
                .bind(client_pubkey_b64)
                .fetch_one(pool)
                .await?
            }
        };
        Ok((row.0.max(0) as u64, row.1.max(0) as u64))
    }

    pub async fn fetch_grpc_peer_profile_kind(
        &self,
        client_pubkey_b64: &str,
    ) -> Result<Option<i16>, DbError> {
        let v = match self {
            Self::Postgres(pool) => {
                sqlx::query_scalar::<_, Option<i16>>(
                    "SELECT connection_kind FROM grpc_peer_profile WHERE client_pubkey_b64 = $1",
                )
                .bind(client_pubkey_b64)
                .fetch_optional(pool)
                .await?
                .flatten()
            }
            Self::Sqlite(pool) => {
                let x: Option<i64> = sqlx::query_scalar(
                    "SELECT connection_kind FROM grpc_peer_profile WHERE client_pubkey_b64 = $1",
                )
                .bind(client_pubkey_b64)
                .fetch_optional(pool)
                .await?;
                x.map(|k| k as i16)
            }
        };
        Ok(v)
    }

    pub async fn fetch_grpc_peer_resource_snapshot(
        &self,
        client_pubkey_b64: &str,
    ) -> Result<Option<GrpcPeerResourceSnapshotRow>, DbError> {
        let q = r#"
            SELECT client_pubkey_b64, cpu_percent, ram_percent, gpu_percent,
                   ram_used_bytes, storage_used_bytes, reported_at
            FROM grpc_peer_resource_snapshot
            WHERE client_pubkey_b64 = $1
        "#;
        let row = match self {
            Self::Postgres(pool) => sqlx::query_as::<_, GrpcPeerResourceSnapshotRow>(q)
                .bind(client_pubkey_b64)
                .fetch_optional(pool)
                .await?,
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, GrpcPeerResourceSnapshotRowSqlite>(q)
                    .bind(client_pubkey_b64)
                    .fetch_optional(pool)
                    .await?
                    .map(Into::into)
            }
        };
        Ok(row)
    }

    pub async fn upsert_peer_display_topology(
        &self,
        client_pubkey_b64: &str,
        stream_capable: bool,
        json_displays: &str,
    ) -> Result<(), DbError> {
        let now = chrono::Utc::now().timestamp_millis();
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO peer_display_topology (client_pubkey_b64, updated_at_ms, stream_capable, json_displays)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (client_pubkey_b64) DO UPDATE SET
                      updated_at_ms = EXCLUDED.updated_at_ms,
                      stream_capable = EXCLUDED.stream_capable,
                      json_displays = EXCLUDED.json_displays
                    "#,
                )
                .bind(client_pubkey_b64)
                .bind(now)
                .bind(stream_capable)
                .bind(json_displays)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                let sc = if stream_capable { 1i64 } else { 0 };
                sqlx::query(
                    r#"
                    INSERT INTO peer_display_topology (client_pubkey_b64, updated_at_ms, stream_capable, json_displays)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (client_pubkey_b64) DO UPDATE SET
                      updated_at_ms = excluded.updated_at_ms,
                      stream_capable = excluded.stream_capable,
                      json_displays = excluded.json_displays
                    "#,
                )
                .bind(client_pubkey_b64)
                .bind(now)
                .bind(sc)
                .bind(json_displays)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn fetch_peer_display_topology(
        &self,
        client_pubkey_b64: &str,
    ) -> Result<Option<(i64, bool, String)>, DbError> {
        let q = r#"
            SELECT updated_at_ms, stream_capable, json_displays
            FROM peer_display_topology
            WHERE client_pubkey_b64 = $1
        "#;
        match self {
            Self::Postgres(pool) => {
                let row: Option<(i64, bool, String)> =
                    sqlx::query_as(q).bind(client_pubkey_b64).fetch_optional(pool).await?;
                Ok(row)
            }
            Self::Sqlite(pool) => {
                let row: Option<(i64, i64, String)> =
                    sqlx::query_as(q).bind(client_pubkey_b64).fetch_optional(pool).await?;
                Ok(row.map(|(a, b, j)| (a, b != 0, j)))
            }
        }
    }

    pub async fn insert_server_resource_benchmark(
        &self,
        cpu_score: i32,
        ram_score: i32,
        storage_score: i32,
        gpu_score: Option<i32>,
        raw_json: &str,
    ) -> Result<i64, DbError> {
        let id = match self {
            Self::Postgres(pool) => {
                let r = sqlx::query_scalar::<_, i64>(
                    r#"
                INSERT INTO server_resource_benchmark (cpu_score, ram_score, storage_score, gpu_score, raw_json)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id
                "#,
                )
                .bind(cpu_score)
                .bind(ram_score)
                .bind(storage_score)
                .bind(gpu_score)
                .bind(raw_json)
                .fetch_one(pool)
                .await?;
                r
            }
            Self::Sqlite(pool) => {
                sqlx::query(
                    r#"
                INSERT INTO server_resource_benchmark (cpu_score, ram_score, storage_score, gpu_score, raw_json)
                VALUES ($1, $2, $3, $4, $5)
                "#,
                )
                .bind(cpu_score)
                .bind(ram_score)
                .bind(storage_score)
                .bind(gpu_score)
                .bind(raw_json)
                .execute(pool)
                .await?;
                sqlx::query_scalar::<_, i64>("SELECT last_insert_rowid()")
                    .fetch_one(pool)
                    .await?
            }
        };
        Ok(id)
    }

    pub async fn fetch_latest_server_resource_benchmark(
        &self,
    ) -> Result<Option<ServerResourceBenchmarkRow>, DbError> {
        let q_pg = r#"
            SELECT id, run_at, cpu_score, ram_score, storage_score, gpu_score, raw_json
            FROM server_resource_benchmark
            ORDER BY id DESC
            LIMIT 1
        "#;
        let q_sl = r#"
            SELECT id, run_at, cpu_score, ram_score, storage_score, gpu_score, raw_json
            FROM server_resource_benchmark
            ORDER BY id DESC
            LIMIT 1
        "#;
        let row = match self {
            Self::Postgres(pool) => sqlx::query_as::<_, ServerResourceBenchmarkRow>(q_pg)
                .fetch_optional(pool)
                .await?,
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, ServerResourceBenchmarkRowSqlite>(q_sl)
                    .fetch_optional(pool)
                    .await?
                    .map(Into::into)
            }
        };
        Ok(row)
    }

    /// Insert a managed proxy session; returns new `session_id`.
    pub async fn insert_grpc_proxy_session(
        &self,
        client_pubkey_b64: &str,
        board_label: &str,
        token_sha256_hex: &str,
        subscription_token: &str,
        expires_at: DateTime<Utc>,
        policy_json: &str,
        wire_mode: Option<i32>,
        wire_config_json: Option<&str>,
        ingress_protocol: Option<i16>,
        ingress_listen_port: Option<i32>,
        ingress_listen_udp_port: Option<i32>,
        ingress_config_json: Option<&str>,
        ingress_tls_json: Option<&str>,
        ingress_template_version: i32,
    ) -> Result<String, DbError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let ver = ingress_template_version.max(1);
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            INSERT INTO grpc_proxy_session (
              session_id, client_pubkey_b64, board_label, token_sha256_hex,
              subscription_token,
              expires_at, policy_json, wire_mode, wire_config_json,
              ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
              ingress_config_json, ingress_tls_json, ingress_template_version
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
                )
                .bind(&session_id)
                .bind(client_pubkey_b64)
                .bind(board_label)
                .bind(token_sha256_hex)
                .bind(subscription_token)
                .bind(expires_at)
                .bind(policy_json)
                .bind(wire_mode.map(|x| x as i16))
                .bind(wire_config_json)
                .bind(ingress_protocol)
                .bind(ingress_listen_port)
                .bind(ingress_listen_udp_port)
                .bind(ingress_config_json)
                .bind(ingress_tls_json)
                .bind(ver)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                let exp = expires_at.to_rfc3339();
                sqlx::query(
                    r#"
            INSERT INTO grpc_proxy_session (
              session_id, client_pubkey_b64, board_label, token_sha256_hex,
              subscription_token,
              expires_at, policy_json, wire_mode, wire_config_json,
              ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
              ingress_config_json, ingress_tls_json, ingress_template_version
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
                )
                .bind(&session_id)
                .bind(client_pubkey_b64)
                .bind(board_label)
                .bind(token_sha256_hex)
                .bind(subscription_token)
                .bind(exp)
                .bind(policy_json)
                .bind(wire_mode)
                .bind(wire_config_json)
                .bind(ingress_protocol.map(|x| x as i64))
                .bind(ingress_listen_port.map(|x| x as i64))
                .bind(ingress_listen_udp_port.map(|x| x as i64))
                .bind(ingress_config_json)
                .bind(ingress_tls_json)
                .bind(ver as i64)
                .execute(pool)
                .await?;
            }
        }
        Ok(session_id)
    }

    pub async fn fetch_grpc_proxy_session_by_token_sha256(
        &self,
        token_sha256_hex: &str,
    ) -> Result<Option<GrpcProxySessionRow>, DbError> {
        let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session WHERE token_sha256_hex = $1
        "#;
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRow>(q)
                    .bind(token_sha256_hex)
                    .fetch_optional(pool)
                    .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRowSqlite>(q)
                    .bind(token_sha256_hex)
                    .fetch_optional(pool)
                    .await?
                    .map(Into::into)
            }
        };
        Ok(row)
    }

    pub async fn fetch_grpc_proxy_session_by_subscription_token(
        &self,
        subscription_token: &str,
    ) -> Result<Option<GrpcProxySessionRow>, DbError> {
        let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session WHERE subscription_token = $1
        "#;
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRow>(q)
                    .bind(subscription_token)
                    .fetch_optional(pool)
                    .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRowSqlite>(q)
                    .bind(subscription_token)
                    .fetch_optional(pool)
                    .await?
                    .map(Into::into)
            }
        };
        Ok(row)
    }

    pub async fn fetch_grpc_proxy_session_by_id(
        &self,
        session_id: &str,
        client_pubkey_b64: &str,
    ) -> Result<Option<GrpcProxySessionRow>, DbError> {
        let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session WHERE session_id = $1 AND client_pubkey_b64 = $2
        "#;
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRow>(q)
                    .bind(session_id)
                    .bind(client_pubkey_b64)
                    .fetch_optional(pool)
                    .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRowSqlite>(q)
                    .bind(session_id)
                    .bind(client_pubkey_b64)
                    .fetch_optional(pool)
                    .await?
                    .map(Into::into)
            }
        };
        Ok(row)
    }

    pub async fn increment_grpc_proxy_session_traffic(
        &self,
        session_id: &str,
        client_pubkey_b64: &str,
        delta_in: u64,
        delta_out: u64,
        active_ms_delta: i64,
        last_activity_at: DateTime<Utc>,
        set_first_open_if_null: Option<DateTime<Utc>>,
    ) -> Result<(), DbError> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
            UPDATE grpc_proxy_session SET
              bytes_in = bytes_in + $1,
              bytes_out = bytes_out + $2,
              active_ms = active_ms + $3,
              last_activity_at = $4,
              first_open_at = COALESCE(first_open_at, $5)
            WHERE session_id = $6 AND client_pubkey_b64 = $7 AND revoked = FALSE
            "#,
                )
                .bind(delta_in as i64)
                .bind(delta_out as i64)
                .bind(active_ms_delta)
                .bind(last_activity_at)
                .bind(set_first_open_if_null)
                .bind(session_id)
                .bind(client_pubkey_b64)
                .execute(pool)
                .await?;
            }
            Self::Sqlite(pool) => {
                let la = last_activity_at.to_rfc3339();
                let fo = set_first_open_if_null.map(|t| t.to_rfc3339());
                sqlx::query(
                    r#"
            UPDATE grpc_proxy_session SET
              bytes_in = bytes_in + $1,
              bytes_out = bytes_out + $2,
              active_ms = active_ms + $3,
              last_activity_at = $4,
              first_open_at = COALESCE(first_open_at, $5)
            WHERE session_id = $6 AND client_pubkey_b64 = $7 AND revoked = 0
            "#,
                )
                .bind(delta_in as i64)
                .bind(delta_out as i64)
                .bind(active_ms_delta)
                .bind(la)
                .bind(fo)
                .bind(session_id)
                .bind(client_pubkey_b64)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn revoke_grpc_proxy_session(
        &self,
        session_id: &str,
        client_pubkey_b64: &str,
    ) -> Result<u64, DbError> {
        let n = match self {
            Self::Postgres(pool) => {
                let r = sqlx::query(
                    r#"UPDATE grpc_proxy_session SET revoked = TRUE WHERE session_id = $1 AND client_pubkey_b64 = $2"#,
                )
                .bind(session_id)
                .bind(client_pubkey_b64)
                .execute(pool)
                .await?;
                r.rows_affected()
            }
            Self::Sqlite(pool) => {
                let r = sqlx::query(
                    r#"UPDATE grpc_proxy_session SET revoked = 1 WHERE session_id = $1 AND client_pubkey_b64 = $2"#,
                )
                .bind(session_id)
                .bind(client_pubkey_b64)
                .execute(pool)
                .await?;
                r.rows_affected()
            }
        };
        Ok(n)
    }

    /// Revoke by session id only (control-api / operator; not exposed on public gRPC).
    pub async fn revoke_grpc_proxy_session_by_id(&self, session_id: &str) -> Result<u64, DbError> {
        let n = match self {
            Self::Postgres(pool) => {
                let r = sqlx::query(
                    r#"UPDATE grpc_proxy_session SET revoked = TRUE WHERE session_id = $1"#,
                )
                .bind(session_id)
                .execute(pool)
                .await?;
                r.rows_affected()
            }
            Self::Sqlite(pool) => {
                let r = sqlx::query(
                    r#"UPDATE grpc_proxy_session SET revoked = 1 WHERE session_id = $1"#,
                )
                .bind(session_id)
                .execute(pool)
                .await?;
                r.rows_affected()
            }
        };
        Ok(n)
    }

    pub async fn fetch_grpc_proxy_session_by_id_only(
        &self,
        session_id: &str,
    ) -> Result<Option<GrpcProxySessionRow>, DbError> {
        let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session WHERE session_id = $1
        "#;
        let row = match self {
            Self::Postgres(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRow>(q)
                    .bind(session_id)
                    .fetch_optional(pool)
                    .await?
            }
            Self::Sqlite(pool) => {
                sqlx::query_as::<_, GrpcProxySessionRowSqlite>(q)
                    .bind(session_id)
                    .fetch_optional(pool)
                    .await?
                    .map(Into::into)
            }
        };
        Ok(row)
    }

    /// Paginated list; `revoked_filter` None = all rows.
    pub async fn list_grpc_proxy_sessions_page(
        &self,
        limit: i64,
        offset: i64,
        revoked_filter: Option<bool>,
    ) -> Result<Vec<GrpcProxySessionRow>, DbError> {
        let limit = limit.max(1).min(500);
        let offset = offset.max(0);
        match (self, revoked_filter) {
            (Self::Postgres(pool), None) => {
                let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
        "#;
                Ok(sqlx::query_as::<_, GrpcProxySessionRow>(q)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await?)
            }
            (Self::Postgres(pool), Some(rev)) => {
                let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session
            WHERE revoked = $3
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
        "#;
                Ok(sqlx::query_as::<_, GrpcProxySessionRow>(q)
                    .bind(limit)
                    .bind(offset)
                    .bind(rev)
                    .fetch_all(pool)
                    .await?)
            }
            (Self::Sqlite(pool), None) => {
                let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session
            ORDER BY created_at DESC
            LIMIT ?1 OFFSET ?2
        "#;
                Ok(sqlx::query_as::<_, GrpcProxySessionRowSqlite>(q)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(pool)
                    .await?
                    .into_iter()
                    .map(Into::into)
                    .collect())
            }
            (Self::Sqlite(pool), Some(rev)) => {
                let rev_i: i64 = if rev { 1 } else { 0 };
                let q = r#"
            SELECT session_id, client_pubkey_b64, board_label, token_sha256_hex, subscription_token,
                   created_at, expires_at, policy_json, bytes_in, bytes_out, active_ms,
                   last_activity_at, first_open_at, revoked, wire_mode, wire_config_json,
                   ingress_protocol, ingress_listen_port, ingress_listen_udp_port,
                   ingress_config_json, ingress_tls_json, ingress_template_version
            FROM grpc_proxy_session
            WHERE revoked = ?3
            ORDER BY created_at DESC
            LIMIT ?1 OFFSET ?2
        "#;
                Ok(sqlx::query_as::<_, GrpcProxySessionRowSqlite>(q)
                    .bind(limit)
                    .bind(offset)
                    .bind(rev_i)
                    .fetch_all(pool)
                    .await?
                    .into_iter()
                    .map(Into::into)
                    .collect())
            }
        }
    }

    pub async fn update_grpc_proxy_session_policy(
        &self,
        session_id: &str,
        client_pubkey_b64: &str,
        policy_json: &str,
        expires_at: DateTime<Utc>,
        update_wire: bool,
        wire_mode: Option<i32>,
        wire_config_json: Option<&str>,
        update_ingress: bool,
        ingress_protocol: Option<i16>,
        ingress_listen_port: Option<i32>,
        ingress_listen_udp_port: Option<i32>,
        ingress_config_json: Option<&str>,
        ingress_tls_json: Option<&str>,
        ingress_template_version: i32,
    ) -> Result<u64, DbError> {
        let ver = ingress_template_version.max(1);
        let n = match self {
            Self::Postgres(pool) => {
                match (update_wire, update_ingress) {
                    (true, true) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2,
                       wire_mode = $5, wire_config_json = $6,
                       ingress_protocol = $7, ingress_listen_port = $8, ingress_listen_udp_port = $9,
                       ingress_config_json = $10, ingress_tls_json = $11, ingress_template_version = $12
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = FALSE"#,
                        )
                        .bind(policy_json)
                        .bind(expires_at)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .bind(wire_mode.map(|x| x as i16))
                        .bind(wire_config_json)
                        .bind(ingress_protocol)
                        .bind(ingress_listen_port)
                        .bind(ingress_listen_udp_port)
                        .bind(ingress_config_json)
                        .bind(ingress_tls_json)
                        .bind(ver)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                    (true, false) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2,
                       wire_mode = $5, wire_config_json = $6
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = FALSE"#,
                        )
                        .bind(policy_json)
                        .bind(expires_at)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .bind(wire_mode.map(|x| x as i16))
                        .bind(wire_config_json)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                    (false, true) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2,
                       ingress_protocol = $5, ingress_listen_port = $6, ingress_listen_udp_port = $7,
                       ingress_config_json = $8, ingress_tls_json = $9, ingress_template_version = $10
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = FALSE"#,
                        )
                        .bind(policy_json)
                        .bind(expires_at)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .bind(ingress_protocol)
                        .bind(ingress_listen_port)
                        .bind(ingress_listen_udp_port)
                        .bind(ingress_config_json)
                        .bind(ingress_tls_json)
                        .bind(ver)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                    (false, false) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = FALSE"#,
                        )
                        .bind(policy_json)
                        .bind(expires_at)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                }
            }
            Self::Sqlite(pool) => {
                let exp = expires_at.to_rfc3339();
                match (update_wire, update_ingress) {
                    (true, true) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2,
                       wire_mode = $5, wire_config_json = $6,
                       ingress_protocol = $7, ingress_listen_port = $8, ingress_listen_udp_port = $9,
                       ingress_config_json = $10, ingress_tls_json = $11, ingress_template_version = $12
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = 0"#,
                        )
                        .bind(policy_json)
                        .bind(&exp)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .bind(wire_mode)
                        .bind(wire_config_json)
                        .bind(ingress_protocol.map(|x| x as i64))
                        .bind(ingress_listen_port.map(|x| x as i64))
                        .bind(ingress_listen_udp_port.map(|x| x as i64))
                        .bind(ingress_config_json)
                        .bind(ingress_tls_json)
                        .bind(ver as i64)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                    (true, false) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2,
                       wire_mode = $5, wire_config_json = $6
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = 0"#,
                        )
                        .bind(policy_json)
                        .bind(&exp)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .bind(wire_mode)
                        .bind(wire_config_json)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                    (false, true) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2,
                       ingress_protocol = $5, ingress_listen_port = $6, ingress_listen_udp_port = $7,
                       ingress_config_json = $8, ingress_tls_json = $9, ingress_template_version = $10
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = 0"#,
                        )
                        .bind(policy_json)
                        .bind(&exp)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .bind(ingress_protocol.map(|x| x as i64))
                        .bind(ingress_listen_port.map(|x| x as i64))
                        .bind(ingress_listen_udp_port.map(|x| x as i64))
                        .bind(ingress_config_json)
                        .bind(ingress_tls_json)
                        .bind(ver as i64)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                    (false, false) => {
                        let r = sqlx::query(
                            r#"UPDATE grpc_proxy_session SET policy_json = $1, expires_at = $2
                       WHERE session_id = $3 AND client_pubkey_b64 = $4 AND revoked = 0"#,
                        )
                        .bind(policy_json)
                        .bind(&exp)
                        .bind(session_id)
                        .bind(client_pubkey_b64)
                        .execute(pool)
                        .await?;
                        r.rows_affected()
                    }
                }
            }
        };
        Ok(n)
    }

    /// Sum stats across non-revoked sessions for this peer.
    pub async fn aggregate_grpc_proxy_sessions_for_pubkey(
        &self,
        client_pubkey_b64: &str,
    ) -> Result<GrpcProxySessionAggregate, DbError> {
        let q_pg = r#"
            SELECT COALESCE(SUM(bytes_in), 0)::bigint, COALESCE(SUM(bytes_out), 0)::bigint,
                   COALESCE(SUM(active_ms), 0)::bigint,
                   MAX(last_activity_at), MAX(created_at), MIN(expires_at), COUNT(*)::bigint
            FROM grpc_proxy_session
            WHERE client_pubkey_b64 = $1 AND revoked = FALSE
        "#;
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query_as::<_, (i64, i64, i64, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, i64)>(q_pg)
                    .bind(client_pubkey_b64)
                    .fetch_one(pool)
                    .await?;
                let (bytes_in, bytes_out, active_ms, last_activity, created_at, expires_at, session_count) = row;
                Ok(GrpcProxySessionAggregate {
                    bytes_in: bytes_in.max(0) as u64,
                    bytes_out: bytes_out.max(0) as u64,
                    active_ms: active_ms.max(0) as u64,
                    last_activity_at: last_activity,
                    created_at_max: created_at,
                    expires_at_min: expires_at,
                    session_count: session_count.max(0) as u64,
                })
            }
            Self::Sqlite(pool) => {
                #[derive(sqlx::FromRow)]
                struct AggSqlite {
                    b1: i64,
                    b2: i64,
                    b3: i64,
                    la: Option<String>,
                    cr: Option<String>,
                    ex: Option<String>,
                    cnt: i64,
                }
                let r: AggSqlite = sqlx::query_as(
                    r#"
            SELECT COALESCE(SUM(bytes_in), 0) as b1, COALESCE(SUM(bytes_out), 0) as b2,
                   COALESCE(SUM(active_ms), 0) as b3,
                   MAX(last_activity_at) as la, MAX(created_at) as cr, MIN(expires_at) as ex, COUNT(*) as cnt
            FROM grpc_proxy_session
            WHERE client_pubkey_b64 = $1 AND revoked = 0
            "#,
                )
                .bind(client_pubkey_b64)
                .fetch_one(pool)
                .await?;
                let parse = |s: Option<String>| {
                    s.and_then(|t| DateTime::parse_from_rfc3339(&t).ok().map(|d| d.with_timezone(&Utc)))
                };
                Ok(GrpcProxySessionAggregate {
                    bytes_in: r.b1.max(0) as u64,
                    bytes_out: r.b2.max(0) as u64,
                    active_ms: r.b3.max(0) as u64,
                    last_activity_at: parse(r.la),
                    created_at_max: parse(r.cr),
                    expires_at_min: parse(r.ex),
                    session_count: r.cnt.max(0) as u64,
                })
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GrpcProxySessionAggregate {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub active_ms: u64,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub created_at_max: Option<DateTime<Utc>>,
    pub expires_at_min: Option<DateTime<Utc>>,
    pub session_count: u64,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GrpcProxySessionRow {
    pub session_id: String,
    pub client_pubkey_b64: String,
    pub board_label: String,
    pub token_sha256_hex: String,
    /// Random secret for public subscription URL (not the session auth token).
    pub subscription_token: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub policy_json: String,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub active_ms: i64,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub first_open_at: Option<DateTime<Utc>>,
    pub revoked: bool,
    /// 1 = VLESS, 2 = Trojan, 3 = VMess (deploy_proto ProxyWireMode).
    /// PostgreSQL `SMALLINT`; map to i32 at API boundaries.
    pub wire_mode: Option<i16>,
    pub wire_config_json: Option<String>,
    /// 1=VLESS, 2=VMess, 3=Trojan, 4=Shadowsocks, 5=SOCKS, 6=Hysteria2; None = public ingress disabled.
    pub ingress_protocol: Option<i16>,
    pub ingress_listen_port: Option<i32>,
    pub ingress_listen_udp_port: Option<i32>,
    pub ingress_config_json: Option<String>,
    pub ingress_tls_json: Option<String>,
    pub ingress_template_version: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GrpcProxySessionRowSqlite {
    session_id: String,
    client_pubkey_b64: String,
    board_label: String,
    token_sha256_hex: String,
    subscription_token: Option<String>,
    created_at: String,
    expires_at: String,
    policy_json: String,
    bytes_in: i64,
    bytes_out: i64,
    active_ms: i64,
    last_activity_at: Option<String>,
    first_open_at: Option<String>,
    revoked: i64,
    wire_mode: Option<i64>,
    wire_config_json: Option<String>,
    ingress_protocol: Option<i64>,
    ingress_listen_port: Option<i64>,
    ingress_listen_udp_port: Option<i64>,
    ingress_config_json: Option<String>,
    ingress_tls_json: Option<String>,
    ingress_template_version: i64,
}

impl From<GrpcProxySessionRowSqlite> for GrpcProxySessionRow {
    fn from(r: GrpcProxySessionRowSqlite) -> Self {
        let parse_dt = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
        };
        let opt = |o: Option<String>| o.map(|t| parse_dt(&t));
        Self {
            session_id: r.session_id,
            client_pubkey_b64: r.client_pubkey_b64,
            board_label: r.board_label,
            token_sha256_hex: r.token_sha256_hex,
            subscription_token: r.subscription_token,
            created_at: parse_dt(&r.created_at),
            expires_at: parse_dt(&r.expires_at),
            policy_json: r.policy_json,
            bytes_in: r.bytes_in,
            bytes_out: r.bytes_out,
            active_ms: r.active_ms,
            last_activity_at: opt(r.last_activity_at),
            first_open_at: opt(r.first_open_at),
            revoked: r.revoked != 0,
            wire_mode: r.wire_mode.map(|x| x as i16),
            wire_config_json: r.wire_config_json,
            ingress_protocol: r.ingress_protocol.map(|x| x as i16),
            ingress_listen_port: r.ingress_listen_port.map(|x| x as i32),
            ingress_listen_udp_port: r.ingress_listen_udp_port.map(|x| x as i32),
            ingress_config_json: r.ingress_config_json,
            ingress_tls_json: r.ingress_tls_json,
            ingress_template_version: r.ingress_template_version.max(1) as i32,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GrpcPeerResourceSnapshotRow {
    pub client_pubkey_b64: String,
    pub cpu_percent: Option<f64>,
    pub ram_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub ram_used_bytes: Option<i64>,
    pub storage_used_bytes: Option<i64>,
    pub reported_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GrpcPeerResourceSnapshotRowSqlite {
    client_pubkey_b64: String,
    cpu_percent: Option<f64>,
    ram_percent: Option<f64>,
    gpu_percent: Option<f64>,
    ram_used_bytes: Option<i64>,
    storage_used_bytes: Option<i64>,
    reported_at: String,
}

impl From<GrpcPeerResourceSnapshotRowSqlite> for GrpcPeerResourceSnapshotRow {
    fn from(r: GrpcPeerResourceSnapshotRowSqlite) -> Self {
        let reported_at = DateTime::parse_from_rfc3339(&r.reported_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Self {
            client_pubkey_b64: r.client_pubkey_b64,
            cpu_percent: r.cpu_percent,
            ram_percent: r.ram_percent,
            gpu_percent: r.gpu_percent,
            ram_used_bytes: r.ram_used_bytes,
            storage_used_bytes: r.storage_used_bytes,
            reported_at,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ServerResourceBenchmarkRow {
    pub id: i64,
    pub run_at: DateTime<Utc>,
    pub cpu_score: i32,
    pub ram_score: i32,
    pub storage_score: i32,
    pub gpu_score: Option<i32>,
    pub raw_json: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ServerResourceBenchmarkRowSqlite {
    id: i64,
    run_at: String,
    cpu_score: i32,
    ram_score: i32,
    storage_score: i32,
    gpu_score: Option<i32>,
    raw_json: String,
}

impl From<ServerResourceBenchmarkRowSqlite> for ServerResourceBenchmarkRow {
    fn from(r: ServerResourceBenchmarkRowSqlite) -> Self {
        let run_at = DateTime::parse_from_rfc3339(&r.run_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Self {
            id: r.id,
            run_at,
            cpu_score: r.cpu_score,
            ram_score: r.ram_score,
            storage_score: r.storage_score,
            gpu_score: r.gpu_score,
            raw_json: r.raw_json,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GrpcSessionKindCount {
    pub kind: String,
    pub event_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GrpcSessionEventRow {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub client_pubkey_b64: Option<String>,
    pub peer_ip: String,
    pub grpc_method: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GrpcSessionPeerAggregateRow {
    pub client_pubkey_b64: String,
    pub last_id: i64,
    pub last_created_at: DateTime<Utc>,
    pub last_peer_ip: String,
    pub last_grpc_method: String,
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

#[cfg(test)]
mod grpc_session_tests {
    use super::DbStore;

    #[tokio::test]
    async fn grpc_session_roundtrip_sqlite() {
        let url = "sqlite::memory:";
        let db = DbStore::connect(url).await.expect("connect");
        db.migrate().await.expect("migrate");
        db.insert_grpc_session_event(
            "tcp_open",
            None,
            "127.0.0.1:9",
            "",
            "ok",
            "conn_id=1",
        )
        .await
        .expect("insert");
        db.insert_grpc_session_event(
            "pair_ok",
            Some("abckey"),
            "127.0.0.1:9",
            "Pair",
            "ok",
            "paired",
        )
        .await
        .expect("insert");
        let page = db
            .fetch_grpc_session_events_page(0, 10)
            .await
            .expect("page");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].kind, "pair_ok");
        assert_eq!(page[1].kind, "tcp_open");
        let agg = db
            .fetch_grpc_session_peer_last_activity()
            .await
            .expect("agg");
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].client_pubkey_b64, "abckey");
    }
}
