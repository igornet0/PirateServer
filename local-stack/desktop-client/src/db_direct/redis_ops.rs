//! Redis: lightweight ping, INFO, and key listing (read-only by policy).

use redis::aio::ConnectionManager;
use std::time::Instant;

/// `rediss://` for TLS, `redis://` for cleartex.
pub fn redis_url(host: &str, port: u16, password: &str) -> String {
    if password.is_empty() {
        format!("redis://{host}:{port}/")
    } else {
        format!(
            "redis://:{pw}@{host}:{port}/",
            pw = urlencoding::encode(password)
        )
    }
}

pub async fn redis_test_latency(host: &str, port: u16, password: &str) -> Result<u64, String> {
    let u = redis_url(host, port, password);
    let client = redis::Client::open(u.as_str()).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let mut con = ConnectionManager::new(client)
        .await
        .map_err(|e| e.to_string())?;
    let _: String = redis::cmd("PING")
        .query_async(&mut con)
        .await
        .map_err(|e| e.to_string())?;
    Ok(t0.elapsed().as_millis() as u64)
}

pub async fn redis_open_manager(
    host: &str,
    port: u16,
    password: &str,
) -> Result<ConnectionManager, String> {
    let u = redis_url(host, port, password);
    let client = redis::Client::open(u.as_str()).map_err(|e| e.to_string())?;
    ConnectionManager::new(client)
        .await
        .map_err(|e| e.to_string())
}

/// Sample keys (expensive on large DBs; Explorer uses for quick peek).
pub async fn redis_sample_keys(
    con: &mut ConnectionManager,
    limit: u32,
) -> Result<Vec<String>, String> {
    let lim = limit.clamp(1, 5000) as isize;
    let mut out: Vec<String> = Vec::new();
    let mut cur = 0i64;
    while (out.len() as isize) < lim {
        let (next, keys): (i64, Vec<String>) = redis::cmd("SCAN")
            .arg(cur)
            .arg("COUNT")
            .arg(50i64)
            .query_async(con)
            .await
            .map_err(|e| e.to_string())?;
        out.extend(keys);
        if next == 0 {
            break;
        }
        cur = next;
    }
    out.truncate(lim as usize);
    Ok(out)
}
