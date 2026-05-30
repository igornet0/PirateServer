//! Future-facing adapter surface for non-PG engines. Implementations live next to the driver
//! (`pg_ops`, `mysql_ops`, `redis_ops`). ClickHouse and Mongo are intentionally not wired in-process yet.

use async_trait::async_trait;

use super::DirectConnectParams;

/// Pluggable “direct” database engine (tree + read-only query paths share session storage in `sessions`).
#[async_trait]
pub trait DirectEngine: Send + Sync {
    /// Human-readable id: `postgres`, `mysql`, `redis`, …
    fn engine_id(&self) -> &'static str;
    /// Connection smoke test, returns RTT in ms.
    async fn test_latency(&self, params: &DirectConnectParams) -> Result<u64, String>;
}

pub struct PgEngine;

#[async_trait]
impl DirectEngine for PgEngine {
    fn engine_id(&self) -> &'static str {
        super::ENGINE_POSTGRES
    }

    async fn test_latency(&self, params: &DirectConnectParams) -> Result<u64, String> {
        super::pg_ops::pg_test_latency(params).await
    }
}

pub struct MysqlEngine;

#[async_trait]
impl DirectEngine for MysqlEngine {
    fn engine_id(&self) -> &'static str {
        super::ENGINE_MYSQL
    }

    async fn test_latency(&self, params: &DirectConnectParams) -> Result<u64, String> {
        super::mysql_ops::mysql_test_latency(params).await
    }
}

pub struct RedisEngine;

#[async_trait]
impl DirectEngine for RedisEngine {
    fn engine_id(&self) -> &'static str {
        super::ENGINE_REDIS
    }

    async fn test_latency(&self, params: &DirectConnectParams) -> Result<u64, String> {
        super::redis_ops::redis_test_latency(&params.host, params.port, &params.password).await
    }
}
