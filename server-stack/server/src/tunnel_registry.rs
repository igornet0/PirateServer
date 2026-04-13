//! Redis: online tunnel registry + wait-queue visibility (optional).

use chrono::Utc;
use redis::AsyncCommands;
use redis::Script;
use std::sync::Arc;
use uuid::Uuid;

const KEY_ONLINE_SET: &str = "proxy:tunnels:online";
const KEY_WAIT_ZSET: &str = "proxy:tunnel:wait";

/// Atomic check SCARD vs max, then SADD session tunnel id (see `register_online`).
const SESSION_SADD_LIMIT: &str = r#"
local max = tonumber(ARGV[1])
local tid = ARGV[2]
if max > 0 then
  if redis.call('SCARD', KEYS[1]) >= max then
    return 0
  end
end
redis.call('SADD', KEYS[1], tid)
return 1
"#;

fn tunnel_hash_key(id: &Uuid) -> String {
    format!("proxy:tunnel:{}", id)
}

#[derive(Debug)]
pub enum RegisterOnlineError {
    SessionDeviceLimit,
    Redis(redis::RedisError),
}

impl std::fmt::Display for RegisterOnlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionDeviceLimit => write!(f, "session concurrent tunnel limit"),
            Self::Redis(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RegisterOnlineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Redis(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct TunnelRedis {
    client: redis::Client,
}

impl TunnelRedis {
    pub fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        Ok(Self { client })
    }

    /// `session_max_concurrent`: `0` = unlimited for this session (no SCARD check).
    /// When there is no `session_id` in `fields`, `session_max_concurrent` is ignored.
    pub async fn register_online(
        &self,
        tunnel_id: &Uuid,
        fields: &[(&str, &str)],
        session_max_concurrent: u32,
    ) -> Result<(), RegisterOnlineError> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(RegisterOnlineError::Redis)?;
        let key = tunnel_hash_key(tunnel_id);
        let mut session_id_opt: Option<&str> = None;
        for (k, v) in fields {
            if *k == "session_id" {
                session_id_opt = Some(*v);
                break;
            }
        }
        for (k, v) in fields {
            let _: () = conn
                .hset::<_, _, _, ()>(&key, *k, *v)
                .await
                .map_err(RegisterOnlineError::Redis)?;
        }
        let _: () = conn
            .sadd::<_, _, ()>(KEY_ONLINE_SET, tunnel_id.to_string())
            .await
            .map_err(RegisterOnlineError::Redis)?;
        if let Some(sid) = session_id_opt {
            if !sid.is_empty() {
                let sess_online = format!("proxy:session:{sid}:tunnels_online");
                let script = Script::new(SESSION_SADD_LIMIT);
                let r: i64 = script
                    .key(&sess_online)
                    .arg(session_max_concurrent as i64)
                    .arg(tunnel_id.to_string())
                    .invoke_async(&mut conn)
                    .await
                    .map_err(RegisterOnlineError::Redis)?;
                if r == 0 {
                    let _: () = conn.del(&key).await.map_err(RegisterOnlineError::Redis)?;
                    let _: () = conn
                        .srem(KEY_ONLINE_SET, tunnel_id.to_string())
                        .await
                        .map_err(RegisterOnlineError::Redis)?;
                    return Err(RegisterOnlineError::SessionDeviceLimit);
                }
                let sess_total = format!("proxy:session:{sid}:tunnels_total");
                let _: () = conn
                    .incr(&sess_total, 1_i64)
                    .await
                    .map_err(RegisterOnlineError::Redis)?;
            }
        }
        Ok(())
    }

    pub async fn update_fields(
        &self,
        tunnel_id: &Uuid,
        fields: &[(&str, &str)],
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let key = tunnel_hash_key(tunnel_id);
        for (k, v) in fields {
            let _: () = conn.hset(&key, *k, *v).await?;
        }
        Ok(())
    }

    pub async fn unregister(&self, tunnel_id: &Uuid) -> Result<(), redis::RedisError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let key = tunnel_hash_key(tunnel_id);
        let sid: Option<String> = conn.hget(&key, "session_id").await?;
        let _: () = conn.del(&key).await?;
        let _: () = conn
            .srem(KEY_ONLINE_SET, tunnel_id.to_string())
            .await?;
        let _: () = conn
            .zrem(KEY_WAIT_ZSET, tunnel_id.to_string())
            .await?;
        if let Some(s) = sid {
            if !s.is_empty() {
                let sess_online = format!("proxy:session:{s}:tunnels_online");
                let _: () = conn.srem(&sess_online, tunnel_id.to_string()).await?;
            }
        }
        Ok(())
    }

    /// Waiting for admission slot (observability). Score = priority; member = tunnel_id.
    pub async fn wait_zadd(&self, tunnel_id: &Uuid, priority: i32) -> Result<(), redis::RedisError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let _: () = conn
            .zadd(
                KEY_WAIT_ZSET,
                tunnel_id.to_string(),
                f64::from(priority),
            )
            .await?;
        Ok(())
    }

    pub async fn wait_zrem(&self, tunnel_id: &Uuid) -> Result<(), redis::RedisError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let _: () = conn
            .zrem(KEY_WAIT_ZSET, tunnel_id.to_string())
            .await?;
        Ok(())
    }
}

/// Best-effort Redis unregister when the tunnel task ends (fire-and-forget async on drop).
pub struct RedisTunnelDropGuard {
    pub redis: Option<Arc<TunnelRedis>>,
    pub id: Uuid,
}

impl Drop for RedisTunnelDropGuard {
    fn drop(&mut self) {
        if let Some(r) = self.redis.take() {
            let id = self.id;
            tokio::spawn(async move {
                if let Err(e) = r.unregister(&id).await {
                    tracing::warn!(error = %e, "redis unregister tunnel");
                }
            });
        }
    }
}

pub fn redis_optional_from_env() -> Option<Arc<TunnelRedis>> {
    let url = std::env::var("DEPLOY_REDIS_URL").ok()?;
    let t = url.trim();
    if t.is_empty() {
        return None;
    }
    match TunnelRedis::connect(t) {
        Ok(r) => {
            tracing::info!("DEPLOY_REDIS_URL: tunnel registry enabled");
            Some(Arc::new(r))
        }
        Err(e) => {
            tracing::warn!(error = %e, "DEPLOY_REDIS_URL: connect failed, tunnel registry disabled");
            None
        }
    }
}

pub fn redis_fields_snapshot(
    tunnel_id: &Uuid,
    session_id: Option<&str>,
    client_pubkey_b64: Option<&str>,
    stream_correlation_id: &str,
    wire_mode: i32,
    priority: i32,
    accum_active_ms: u64,
    bytes_in: u64,
    bytes_out: u64,
    last_checkpoint_active_ms: u64,
) -> Vec<(String, String)> {
    let now = Utc::now().to_rfc3339();
    let mut v = vec![
        ("tunnel_id".into(), tunnel_id.to_string()),
        ("updated_at".into(), now),
        (
            "stream_correlation_id".into(),
            stream_correlation_id.to_string(),
        ),
        ("wire_mode".into(), wire_mode.to_string()),
        ("priority".into(), priority.to_string()),
        ("accum_active_ms".into(), accum_active_ms.to_string()),
        ("bytes_in".into(), bytes_in.to_string()),
        ("bytes_out".into(), bytes_out.to_string()),
        (
            "last_checkpoint_active_ms".into(),
            last_checkpoint_active_ms.to_string(),
        ),
    ];
    if let Some(s) = session_id {
        v.push(("session_id".into(), s.to_string()));
    }
    if let Some(pk) = client_pubkey_b64 {
        v.push(("client_pubkey_b64".into(), pk.to_string()));
    }
    v
}
