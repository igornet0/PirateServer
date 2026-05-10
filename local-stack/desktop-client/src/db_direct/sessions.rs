//! One `PgPool` per active direct connection id (plus other engines in `EngineSlot`).

use redis::aio::ConnectionManager;
use sqlx::mysql::MySqlPool;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static MANAGER: OnceLock<Mutex<SessionMap>> = OnceLock::new();

struct SessionMap {
    slots: HashMap<String, EngineSlot>,
}

pub enum EngineSlot {
    Postgres(PgPool),
    Mysql(MySqlPool),
    Redis(ConnectionManager),
}

fn map_lock() -> &'static Mutex<SessionMap> {
    MANAGER.get_or_init(|| {
        Mutex::new(SessionMap {
            slots: HashMap::new(),
        })
    })
}

pub fn session_close(session_id: &str) -> Result<(), String> {
    let mut g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    if let Some(slot) = g.slots.remove(session_id) {
        drop(g);
        session_close_slot(slot);
    }
    Ok(())
}

fn session_close_slot(slot: EngineSlot) {
    match slot {
        EngineSlot::Postgres(p) => {
            tokio::spawn(async move {
                p.close().await;
            });
        }
        EngineSlot::Mysql(p) => {
            tokio::spawn(async move {
                p.close().await;
            });
        }
        EngineSlot::Redis(_m) => {
            // `ConnectionManager` drops open connections; nothing else required.
        }
    }
}

pub fn session_put_postgres(session_id: String, pool: PgPool) -> Result<(), String> {
    let mut g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    if let Some(old) = g.slots.insert(session_id, EngineSlot::Postgres(pool)) {
        session_close_slot(old);
    }
    Ok(())
}

pub fn session_put_mysql(session_id: String, pool: MySqlPool) -> Result<(), String> {
    let mut g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    if let Some(old) = g.slots.insert(session_id, EngineSlot::Mysql(pool)) {
        session_close_slot(old);
    }
    Ok(())
}

/// Clone is cheap for `PgPool`.
pub async fn pg_pool_for(session_id: &str) -> Result<PgPool, String> {
    let g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    match g.slots.get(session_id) {
        Some(EngineSlot::Postgres(p)) => Ok(p.clone()),
        Some(EngineSlot::Mysql(_)) | Some(EngineSlot::Redis(_)) => {
            Err("session is not PostgreSQL".into())
        }
        None => Err("session not found; open a connection first".into()),
    }
}

pub async fn mysql_pool_for(session_id: &str) -> Result<MySqlPool, String> {
    let g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    match g.slots.get(session_id) {
        Some(EngineSlot::Mysql(p)) => Ok(p.clone()),
        Some(EngineSlot::Postgres(_)) | Some(EngineSlot::Redis(_)) => {
            Err("session is not MySQL".into())
        }
        None => Err("session not found; open a connection first".into()),
    }
}

pub async fn redis_manager_for(session_id: &str) -> Result<ConnectionManager, String> {
    let g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    match g.slots.get(session_id) {
        Some(EngineSlot::Redis(m)) => Ok(m.clone()),
        Some(EngineSlot::Postgres(_)) | Some(EngineSlot::Mysql(_)) => {
            Err("session is not Redis".into())
        }
        None => Err("session not found; open a connection first".into()),
    }
}

pub fn pool_options() -> sqlx::pool::PoolOptions<sqlx::Postgres> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(15))
        .idle_timeout(Some(Duration::from_secs(600)))
        .max_lifetime(Some(Duration::from_secs(3600)))
}

pub fn mysql_pool_options() -> sqlx::pool::PoolOptions<sqlx::MySql> {
    sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(15))
        .idle_timeout(Some(Duration::from_secs(600)))
        .max_lifetime(Some(Duration::from_secs(3600)))
}

/// For `open` we use `session_id == profile_id` (one tab = one profile connection).
pub async fn open_postgres_session(
    session_id: String,
    opts: sqlx::postgres::PgConnectOptions,
) -> Result<(), String> {
    let pool = pool_options()
        .connect_with(opts)
        .await
        .map_err(|e| e.to_string())?;
    session_put_postgres(session_id, pool)
}

pub async fn open_mysql_session(
    session_id: String,
    opts: sqlx::mysql::MySqlConnectOptions,
) -> Result<(), String> {
    let pool = mysql_pool_options()
        .connect_with(opts)
        .await
        .map_err(|e| e.to_string())?;
    session_put_mysql(session_id, pool)
}

pub fn session_put_redis(session_id: String, manager: ConnectionManager) -> Result<(), String> {
    let mut g = map_lock().lock().map_err(|_| "session lock".to_string())?;
    if let Some(old) = g.slots.insert(session_id, EngineSlot::Redis(manager)) {
        session_close_slot(old);
    }
    Ok(())
}

pub async fn open_redis_session(
    session_id: String,
    manager: ConnectionManager,
) -> Result<(), String> {
    session_put_redis(session_id, manager)
}
