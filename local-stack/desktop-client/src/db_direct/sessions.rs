//! One `PgPool` per active direct connection id (plus other engines in `EngineSlot`).

use redis::aio::ConnectionManager;
use sqlx::mysql::MySqlPool;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SESSION_IDLE_TTL: Duration = Duration::from_secs(900);

static MANAGER: OnceLock<Mutex<SessionMap>> = OnceLock::new();

struct SessionEntry {
    slot: EngineSlot,
    last_used: Instant,
}

struct SessionMap {
    slots: HashMap<String, SessionEntry>,
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

async fn evict_idle_sessions() {
    let mut g = map_lock().lock().await;
    let now = Instant::now();
    let stale: Vec<String> = g
        .slots
        .iter()
        .filter(|(_, e)| now.duration_since(e.last_used) > SESSION_IDLE_TTL)
        .map(|(k, _)| k.clone())
        .collect();
    for id in stale {
        if let Some(entry) = g.slots.remove(&id) {
            session_close_slot(entry.slot);
        }
    }
}

pub async fn session_close(session_id: &str) -> Result<(), String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    if let Some(entry) = g.slots.remove(session_id) {
        session_close_slot(entry.slot);
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
        EngineSlot::Redis(_m) => {}
    }
}

pub async fn session_put_postgres(session_id: String, pool: PgPool) -> Result<(), String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    if let Some(old) = g.slots.insert(
        session_id,
        SessionEntry {
            slot: EngineSlot::Postgres(pool),
            last_used: Instant::now(),
        },
    ) {
        session_close_slot(old.slot);
    }
    Ok(())
}

pub async fn session_put_mysql(session_id: String, pool: MySqlPool) -> Result<(), String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    if let Some(old) = g.slots.insert(
        session_id,
        SessionEntry {
            slot: EngineSlot::Mysql(pool),
            last_used: Instant::now(),
        },
    ) {
        session_close_slot(old.slot);
    }
    Ok(())
}

pub async fn pg_pool_for(session_id: &str) -> Result<PgPool, String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    let out = match g.slots.get(session_id) {
        Some(entry) => match &entry.slot {
            EngineSlot::Postgres(p) => Ok(p.clone()),
            EngineSlot::Mysql(_) | EngineSlot::Redis(_) => Err("session is not PostgreSQL".into()),
        },
        None => Err("session not found; open a connection first".into()),
    };
    if out.is_ok() {
        if let Some(e) = g.slots.get_mut(session_id) {
            e.last_used = Instant::now();
        }
    }
    out
}

pub async fn mysql_pool_for(session_id: &str) -> Result<MySqlPool, String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    let out = match g.slots.get(session_id) {
        Some(entry) => match &entry.slot {
            EngineSlot::Mysql(p) => Ok(p.clone()),
            EngineSlot::Postgres(_) | EngineSlot::Redis(_) => Err("session is not MySQL".into()),
        },
        None => Err("session not found; open a connection first".into()),
    };
    if out.is_ok() {
        if let Some(e) = g.slots.get_mut(session_id) {
            e.last_used = Instant::now();
        }
    }
    out
}

pub async fn redis_manager_for(session_id: &str) -> Result<ConnectionManager, String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    let out = match g.slots.get(session_id) {
        Some(entry) => match &entry.slot {
            EngineSlot::Redis(m) => Ok(m.clone()),
            EngineSlot::Postgres(_) | EngineSlot::Mysql(_) => Err("session is not Redis".into()),
        },
        None => Err("session not found; open a connection first".into()),
    };
    if out.is_ok() {
        if let Some(e) = g.slots.get_mut(session_id) {
            e.last_used = Instant::now();
        }
    }
    out
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

pub async fn open_postgres_session(
    session_id: String,
    opts: sqlx::postgres::PgConnectOptions,
) -> Result<(), String> {
    let pool = pool_options()
        .connect_with(opts)
        .await
        .map_err(|e| e.to_string())?;
    session_put_postgres(session_id, pool).await
}

pub async fn open_mysql_session(
    session_id: String,
    opts: sqlx::mysql::MySqlConnectOptions,
) -> Result<(), String> {
    let pool = mysql_pool_options()
        .connect_with(opts)
        .await
        .map_err(|e| e.to_string())?;
    session_put_mysql(session_id, pool).await
}

pub async fn session_put_redis(session_id: String, manager: ConnectionManager) -> Result<(), String> {
    evict_idle_sessions().await;
    let mut g = map_lock().lock().await;
    if let Some(old) = g.slots.insert(
        session_id,
        SessionEntry {
            slot: EngineSlot::Redis(manager),
            last_used: Instant::now(),
        },
    ) {
        session_close_slot(old.slot);
    }
    Ok(())
}

pub async fn open_redis_session(
    session_id: String,
    manager: ConnectionManager,
) -> Result<(), String> {
    session_put_redis(session_id, manager).await
}
