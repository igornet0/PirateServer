//! PostgreSQL statistics queries for the Explorer stats screen (SQL source only).

use serde::Serialize;
use sqlx::PgPool;
use std::time::Instant;

use super::QueryResultView;
use super::pg_ops::pg_run_readonly_sql;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PgStatsBundle {
    pub source_note: String,
    pub database_sizes: Option<QueryResultView>,
    pub connection_summary: Option<QueryResultView>,
    pub top_activity: Option<QueryResultView>,
    pub statements_ext: Option<QueryResultView>,
    pub rtt_ms: u64,
}

/// Latency/heartbeat: round-trip to run `SELECT 1`.
pub async fn pg_heartbeat_rtt_ms(pool: &PgPool) -> u64 {
    let t0 = Instant::now();
    if sqlx::query("SELECT 1").fetch_one(pool).await.is_err() {
        return 0;
    }
    t0.elapsed().as_millis() as u64
}

const NOTE: &str = "Data from PostgreSQL system catalogs in this connection only. Host CPU/RAM and OS metrics are not available without a host agent; this is the same class of limitation as DBeaver when querying `pg_stat_*` alone.";

pub async fn pg_stats_bundle(pool: &PgPool) -> PgStatsBundle {
    let rtt_ms = pg_heartbeat_rtt_ms(pool).await;

    let q_fin = "SELECT d.datname::text AS database,
         pg_size_pretty(pg_database_size(d.oid))::text AS size_pretty,
         pg_database_size(d.oid) AS size_bytes
      FROM pg_database d
      WHERE d.datistemplate = false
      ORDER BY size_bytes DESC NULLS LAST LIMIT 20";
    let database_sizes = pg_run_readonly_sql(pool, q_fin, 50).await.ok();
    let q_conn = "SELECT state, count(*)::bigint AS n FROM pg_stat_activity GROUP BY state ORDER BY n DESC";
    let connection_summary = pg_run_readonly_sql(pool, q_conn, 20).await.ok();
    let q_act = "SELECT pid, usename::text, application_name, client_addr::text, state, query_start, left(query, 200) AS query_snippet
        FROM pg_stat_activity
        WHERE state IS NOT NULL
        ORDER BY query_start NULLS LAST LIMIT 15";
    let top_activity = pg_run_readonly_sql(pool, q_act, 20).await.ok();

    let statements_ext = if pg_stat_statements_available(pool).await {
        let q2 = "SELECT queryid::text, calls, mean_exec_time::text, total_exec_time::text, left(query, 120) AS query_snippet
            FROM pg_stat_statements
            ORDER BY mean_exec_time DESC NULLS LAST LIMIT 15";
        pg_run_readonly_sql(pool, q2, 20).await.ok()
    } else {
        None
    };

    PgStatsBundle {
        source_note: NOTE.into(),
        database_sizes,
        connection_summary,
        top_activity,
        statements_ext,
        rtt_ms,
    }
}

async fn pg_stat_statements_available(pool: &PgPool) -> bool {
    sqlx::query("SELECT 1 FROM pg_stat_statements LIMIT 1")
        .fetch_optional(pool)
        .await
        .is_ok()
}
