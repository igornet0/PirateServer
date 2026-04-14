//! Read per-session tunnel counters from the same Redis used by deploy-server (`DEPLOY_REDIS_URL`).

use redis::AsyncCommands;
use std::collections::HashMap;

/// For each session id: (online tunnel count, cumulative opens since counters existed in Redis).
pub async fn fetch_session_tunnel_stats(
    client: &redis::Client,
    session_ids: &[String],
) -> Result<HashMap<String, (u64, u64)>, redis::RedisError> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut conn = client.get_multiplexed_async_connection().await?;
    let mut out = HashMap::with_capacity(session_ids.len());
    for sid in session_ids {
        let online_key = format!("proxy:session:{sid}:tunnels_online");
        let total_key = format!("proxy:session:{sid}:tunnels_total");
        let online: u64 = conn.scard(&online_key).await.unwrap_or(0);
        let total: u64 = match conn.get::<_, Option<u64>>(&total_key).await {
            Ok(Some(n)) => n,
            _ => 0,
        };
        out.insert(sid.clone(), (online, total));
    }
    Ok(out)
}
