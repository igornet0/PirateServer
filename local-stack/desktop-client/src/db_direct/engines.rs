//! Engine identifiers and adapter hooks (extensible).

pub const ENGINE_POSTGRES: &str = "postgres";
pub const ENGINE_MYSQL: &str = "mysql";
pub const ENGINE_REDIS: &str = "redis";
pub const ENGINE_CLICKHOUSE: &str = "clickhouse";
pub const ENGINE_MONGO: &str = "mongo";

/// Whether this engine is implemented for **direct** Explorer sessions in this build.
pub fn is_direct_engine_implemented(engine: &str) -> bool {
    matches!(
        engine,
        ENGINE_POSTGRES | ENGINE_MYSQL | ENGINE_REDIS // ClickHouse and Mongo: UI may show a clear “not available locally yet”
                                                      // without failing profile storage.
    )
}

/// Placeholder for future `clickhouse` HTTP driver in the desktop process.
pub fn clickhouse_not_implemented() -> String {
    "Direct ClickHouse in the desktop app is not wired yet; use the host DB viewer (control-api) for managed hosts, or a plain HTTP client to your CH endpoint."
        .into()
}

/// Placeholder for future MongoDB driver in the desktop process.
pub fn mongo_not_implemented() -> String {
    "Direct MongoDB in the desktop app is not wired yet; use the host DB viewer (control-api) for managed hosts, or a dedicated tool."
        .into()
}
