//! HTTP CONNECT proxy checks: smoke, download speed, parallel connection estimate.

use reqwest::Proxy;
use serde::Serialize;
use std::time::Instant;

/// Default upstream URL when `--upstream-url` is omitted (`PIRATE_PROXY_TEST_UPSTREAM` or bench-upstream-style path).
pub fn default_upstream_url() -> String {
    std::env::var("PIRATE_PROXY_TEST_UPSTREAM").unwrap_or_else(|_| {
        "http://127.0.0.1:9000/size?bytes=262144".to_string()
    })
}

fn build_client(proxy_url: &str, timeout: std::time::Duration) -> Result<reqwest::Client, Box<dyn std::error::Error>> {
    let p = Proxy::http(proxy_url)?;
    Ok(reqwest::Client::builder()
        .proxy(p)
        .timeout(timeout)
        .pool_max_idle_per_host(0)
        .build()?)
}

/// Single GET through proxy; returns bytes read and status.
async fn fetch_through_proxy(
    client: &reqwest::Client,
    url: &str,
) -> Result<(reqwest::StatusCode, u64), reqwest::Error> {
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let bytes = resp.bytes().await.map(|b| b.len() as u64)?;
    Ok((status, bytes))
}

#[derive(Debug, Clone)]
pub struct TestProxyOptions {
    pub proxy_url: String,
    pub upstream_url: String,
    /// Payload hint for speed test (appended as `bytes` query if URL has no `bytes=`).
    pub bytes: u64,
    pub timeout_secs: u64,
    pub max_connect_cap: u32,
    pub min_success_rate: f64,
    pub run_speed: bool,
    pub run_max_connect: bool,
    pub json: bool,
}

impl TestProxyOptions {
    pub fn run_all(&self) -> bool {
        !self.run_speed && !self.run_max_connect
    }
}

#[derive(Serialize)]
pub struct TestProxyJson {
    pub smoke_ok: bool,
    pub smoke_status: Option<u16>,
    pub smoke_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_max_parallel: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_connect_trials_note: Option<String>,
}

fn speed_url(base: &str, bytes: u64) -> String {
    if base.contains("bytes=") || base.contains("size?") {
        return base.to_string();
    }
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}bytes={bytes}")
}

pub async fn run_proxy_tests(opts: TestProxyOptions) -> Result<(), Box<dyn std::error::Error>> {
    let timeout = std::time::Duration::from_secs(opts.timeout_secs.max(1));
    let run_speed = opts.run_speed || opts.run_all();
    let run_max = opts.run_max_connect || opts.run_all();

    let client = build_client(&opts.proxy_url, timeout)?;

    let mut out = TestProxyJson {
        smoke_ok: false,
        smoke_status: None,
        smoke_error: None,
        speed_mbps: None,
        speed_latency_ms: None,
        speed_bytes: None,
        estimated_max_parallel: None,
        max_connect_trials_note: None,
    };

    // --- Smoke ---
    match fetch_through_proxy(&client, &opts.upstream_url).await {
        Ok((status, n)) if status.is_success() => {
            out.smoke_ok = true;
            out.smoke_status = Some(status.as_u16());
            if !opts.json {
                println!(
                    "smoke: proxy {} -> upstream OK (HTTP {}, {} bytes)",
                    opts.proxy_url,
                    status.as_u16(),
                    n
                );
            }
        }
        Ok((status, n)) => {
            out.smoke_status = Some(status.as_u16());
            out.smoke_error = Some(format!("HTTP {}", status.as_u16()));
            if !opts.json {
                eprintln!(
                    "smoke: proxy {} -> upstream HTTP {} ({} bytes)",
                    opts.proxy_url,
                    status,
                    n
                );
            }
        }
        Err(e) => {
            out.smoke_error = Some(e.to_string());
            if !opts.json {
                eprintln!(
                    "smoke: failed (is `pirate board` running on {}?): {}",
                    opts.proxy_url, e
                );
            }
        }
    }

    let smoke_required = opts.run_all() || opts.run_speed;
    if !out.smoke_ok && smoke_required {
        return Err(
            format!(
                "smoke failed: {}",
                out.smoke_error
                    .as_deref()
                    .unwrap_or("non-success HTTP status")
            )
            .into(),
        );
    }

    // --- Speed (through CONNECT proxy only; not gRPC ConnectionProbe) ---
    if run_speed {
        let url = speed_url(&opts.upstream_url, opts.bytes);
        let mut best_mbps = 0_f64;
        let mut sum_ms = 0_f64;
        let mut total_bytes = 0;
        const RUNS: usize = 3;
        for _ in 0..RUNS {
            let t0 = Instant::now();
            let r = fetch_through_proxy(&client, &url).await;
            let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok((st, n)) if st.is_success() => {
                    sum_ms += elapsed;
                    total_bytes += n;
                    let mbps = if elapsed > 0.0 {
                        (n as f64 * 8.0) / 1_000_000.0 / (elapsed / 1000.0)
                    } else {
                        0.0
                    };
                    if mbps > best_mbps {
                        best_mbps = mbps;
                    }
                }
                Ok((st, _)) => {
                    if !opts.json {
                        eprintln!("speed: non-success HTTP {}", st);
                    }
                }
                Err(e) => {
                    if !opts.json {
                        eprintln!("speed: request error: {e}");
                    }
                }
            }
        }
        let avg_ms = if RUNS > 0 {
            sum_ms / RUNS as f64
        } else {
            0.0
        };
        let avg_bytes = if RUNS > 0 {
            total_bytes / RUNS as u64
        } else {
            0
        };
        out.speed_mbps = Some(best_mbps);
        out.speed_latency_ms = Some(avg_ms);
        out.speed_bytes = Some(avg_bytes);
        if !opts.json {
            println!(
                "speed (via proxy, {} runs): best≈{:.2} Mbps, avg latency≈{:.2} ms, ~{} bytes/req",
                RUNS, best_mbps, avg_ms, avg_bytes
            );
        }
    }

    // --- Max parallel (binary search on success rate) ---
    if run_max {
        let note = "empirical estimate; depends on upstream, client PIRATE_MAX_CONCURRENT_TUNNELS, server DEPLOY_PROXY_MAX_CONCURRENT_TUNNELS, and network";
        out.max_connect_trials_note = Some(note.to_string());
        let cap = opts.max_connect_cap.max(1);
        let url = speed_url(&opts.upstream_url, opts.bytes.min(262144));

        let mut lo: u32 = 1;
        let mut hi: u32 = cap;
        let mut best: u32 = 0;

        while lo <= hi {
            let mid = (lo + hi) / 2;
            let ok = trial_parallel_rate(&client, &url, mid, opts.min_success_rate).await;
            if ok {
                best = mid;
                lo = mid.saturating_add(1);
            } else {
                hi = mid.saturating_sub(1);
            }
        }

        out.estimated_max_parallel = Some(best);
        if !opts.json {
            println!(
                "max_connect (estimate): ~{} parallel successful requests (cap={}, min_success_rate={:.2})",
                best, cap, opts.min_success_rate
            );
            println!("note: {}", note);
        }
    }

    if opts.json {
        println!("{}", serde_json::to_string(&out)?);
    }

    Ok(())
}

async fn trial_parallel_rate(
    client: &reqwest::Client,
    url: &str,
    n: u32,
    min_rate: f64,
) -> bool {
    if n == 0 {
        return false;
    }
    let url = url.to_string();
    let client = client.clone();
    let mut handles = Vec::new();
    for _ in 0..n {
        let c = client.clone();
        let u = url.clone();
        handles.push(tokio::spawn(async move {
            match c.get(&u).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        return false;
                    }
                    match resp.bytes().await {
                        Ok(_) => true,
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            }
        }));
    }
    let mut ok = 0u32;
    for h in handles {
        if let Ok(true) = h.await {
            ok += 1;
        }
    }
    let rate = ok as f64 / n as f64;
    rate >= min_rate
}
