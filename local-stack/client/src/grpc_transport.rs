//! gRPC `Channel` construction: TLS, SNI, HTTP/2 keep-alive, optional stealth metadata on requests.

use crate::settings::BoardConfig;
use crate::tls_profile::client_tls_config;
use std::time::Duration;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::transport::{Channel, Endpoint};
use tonic::Request;

fn scheme_host_port(endpoint: &str) -> Result<(bool, String, Option<u16>), String> {
    let e = endpoint.trim();
    let (https, rest) = if let Some(r) = e.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = e.strip_prefix("http://") {
        (false, r)
    } else {
        (false, e)
    };
    let rest = rest.trim_end_matches('/');
    if rest.starts_with('[') {
        let end = rest.find(']').ok_or("invalid IPv6 in endpoint")?;
        let host = rest[1..end].to_string();
        let after = rest[end + 1..].trim_start_matches(':');
        let port = if after.is_empty() {
            None
        } else {
            Some(after.parse().map_err(|_| "invalid port")?)
        };
        return Ok((https, host, port));
    }
    if let Some((h, p)) = rest.rsplit_once(':') {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            return Ok((https, h.to_string(), Some(p.parse().map_err(|_| "invalid port")?)));
        }
    }
    Ok((https, rest.to_string(), None))
}

/// Cache key for pooled channels: endpoint + TLS parameters that affect the handshake.
pub fn grpc_channel_cache_key(endpoint: &str, board: &BoardConfig) -> String {
    format!(
        "{}|p={}|sni={}|ka={:?}",
        endpoint.trim(),
        board.tls_profile.as_deref().unwrap_or(""),
        board.grpc_tls_sni.as_deref().unwrap_or(""),
        board.grpc_keep_alive_interval_secs,
    )
}

fn tls_domain_for_endpoint(endpoint: &str, board: &BoardConfig) -> Result<String, String> {
    if let Some(s) = board.grpc_tls_sni.as_ref() {
        let t = s.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    let (_, host, _) = scheme_host_port(endpoint)?;
    if host.is_empty() {
        return Err("endpoint has no host for TLS SNI".into());
    }
    Ok(host)
}

/// Build a lazy `Channel` for the given gRPC endpoint and board TLS options.
pub fn endpoint_channel(endpoint: &str, board: &BoardConfig) -> Result<Channel, String> {
    if board.experimental_http3 {
        #[cfg(not(feature = "experimental-quic"))]
        {
            return Err(
                "experimental_http3 is set but this binary was built without `experimental-quic`"
                    .into(),
            );
        }
        #[cfg(feature = "experimental-quic")]
        {
            return Err("HTTP/3 to DeployService is not implemented yet".into());
        }
    }

    let e = endpoint.trim();
    if e.is_empty() {
        return Err("empty gRPC endpoint".into());
    }

    let (https, _, _) = scheme_host_port(e)?;
    if board.stealth_mode && !https {
        return Err("stealth_mode requires https:// gRPC endpoint".into());
    }

    let mut ep = Endpoint::from_shared(e.to_string()).map_err(|x| x.to_string())?;

    if let Some(secs) = board.grpc_keep_alive_interval_secs {
        let d = Duration::from_secs(secs.max(1));
        ep = ep
            .http2_keep_alive_interval(d)
            .keep_alive_timeout(d.saturating_mul(2))
            .keep_alive_while_idle(true);
    }

    if https {
        let domain = tls_domain_for_endpoint(e, board)?;
        let tls = client_tls_config(board).domain_name(domain);
        ep = ep.tls_config(tls).map_err(|x| x.to_string())?;
    }

    Ok(ep.connect_lazy())
}

/// When `stealth_mode`, attach browser-like metadata (does not change `:authority`).
pub fn apply_stealth_metadata<T>(board: &BoardConfig, req: &mut Request<T>) -> Result<(), String> {
    if !board.stealth_mode {
        return Ok(());
    }
    let map = req.metadata_mut();
    let ua: MetadataValue<Ascii> = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
        .parse()
        .map_err(|_| "metadata User-Agent")?;
    map.insert("user-agent", ua);
    let accept: MetadataValue<Ascii> =
        "application/grpc".parse().map_err(|_| "metadata accept")?;
    map.insert("accept", accept);
    Ok(())
}

/// Random delay before starting the tunnel RPC when stealth jitter is configured.
pub async fn stealth_jitter_before_rpc(board: &BoardConfig) {
    let Some(ref st) = board.stealth else {
        return;
    };
    if !board.stealth_mode {
        return;
    }
    let lo = st.jitter_ms_min.min(st.jitter_ms_max);
    let hi = st.jitter_ms_max.max(st.jitter_ms_min);
    if hi == 0 {
        return;
    }
    let span = hi.saturating_sub(lo).saturating_add(1);
    let ms = lo + (rand::random::<u64>() % span);
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
