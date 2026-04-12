//! Local HTTP CONNECT proxy → gRPC `ProxyTunnel` or direct TCP when bypass matches.

use crate::bypass::BypassMatcher;
use crate::config::normalize_endpoint;
use crate::connection_manager::ConnectionManager;
use crate::metrics_collector::TunnelMetrics;
use crate::routing::resolve_board_for_host;
use crate::settings::{BoardConfig, SettingsSnapshot};
use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::{proxy_client_msg, proxy_server_msg, ProxyClientMsg, ProxyOpen};
use wire_protocol::parse_subscription_uri;
use wire_protocol::wire_tunnel_first_chunk;
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use futures_util::StreamExt;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;

const MAX_HEADER_READ: usize = 64 * 1024;
const MAX_CHUNK: usize = 256 * 1024;

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

async fn read_http_headers(
    sock: &mut TcpStream,
) -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if buf.len() > MAX_HEADER_READ {
            return Err("HTTP headers too large".into());
        }
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Err("connection closed before headers".into());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = find_headers_end(&buf) {
            let tail = buf[end..].to_vec();
            return Ok((buf[..end].to_vec(), tail));
        }
    }
}

fn parse_connect_target(target: &str) -> Option<(String, u16)> {
    let t = target.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with('[') {
        let end = t.find(']')?;
        let host = t[1..end].to_string();
        let rest = t[end + 1..].trim_start_matches(':');
        let port: u16 = if rest.is_empty() {
            443
        } else {
            rest.parse().ok()?
        };
        return Some((host, port));
    }
    if let Some((h, p)) = t.rsplit_once(':') {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            return Some((h.to_string(), p.parse().ok()?));
        }
    }
    Some((t.to_string(), 443))
}

fn parse_connect_request(header_bytes: &[u8]) -> Result<(String, u16), Box<dyn std::error::Error>> {
    let text = std::str::from_utf8(header_bytes)?;
    let mut lines = text.lines();
    let first = lines.next().ok_or("empty request")?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or("bad request line")?;
    if !method.eq_ignore_ascii_case("CONNECT") {
        return Err("only HTTP CONNECT is supported (set HTTPS_PROXY to this listener)".into());
    }
    let target = parts.next().ok_or("missing CONNECT target")?;
    parse_connect_target(target).ok_or_else(|| "invalid CONNECT target".into())
}

fn board_config(snap: &SettingsSnapshot, board_id: &str) -> (BoardConfig, String) {
    let def = snap.data.default_board.trim();
    let id = if board_id.is_empty() {
        if def.is_empty() {
            "default"
        } else {
            def
        }
    } else {
        board_id
    };
    let cfg = snap
        .data
        .boards
        .get(id)
        .cloned()
        .unwrap_or_default();
    (cfg, id.to_string())
}

fn resolve_grpc_url(board: &BoardConfig, cli_endpoint: &str, connection_url: &str) -> String {
    if let Some(ref u) = board.url {
        let t = u.trim();
        if !t.is_empty() {
            return normalize_endpoint(t);
        }
    }
    let c = cli_endpoint.trim();
    if !c.is_empty() {
        return normalize_endpoint(c);
    }
    normalize_endpoint(connection_url)
}

fn session_token_for_board(board: &BoardConfig, override_tok: Option<&str>) -> Option<String> {
    if let Some(t) = override_tok {
        let s = t.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    board
        .session_token
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Run local HTTP proxy: each CONNECT is tunneled over gRPC or direct when bypass matches.
pub async fn run_board(
    listen: &str,
    grpc_endpoint: &str,
    connection_url: &str,
    sk: &SigningKey,
    project_id: &str,
    board_id: &str,
    settings: Arc<RwLock<SettingsSnapshot>>,
    pool: Arc<ConnectionManager>,
    session_token_cli: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(listen).await?;
    eprintln!("pirate board listening on {listen}");
    loop {
        let (sock, peer) = listener.accept().await?;
        let grpc_endpoint = grpc_endpoint.to_string();
        let connection_url = connection_url.to_string();
        let sk = sk.clone();
        let settings = settings.clone();
        let pool = pool.clone();
        let project_id = project_id.to_string();
        let board_id = board_id.to_string();
        let tok = session_token_cli.map(|s| s.to_string());
        tokio::spawn(async move {
            if let Err(e) = handle_one_connection(
                sock,
                &grpc_endpoint,
                &connection_url,
                &sk,
                &project_id,
                &board_id,
                settings,
                pool,
                tok.as_deref(),
            )
            .await
            {
                eprintln!("board {peer}: {e}");
            }
        });
    }
}

async fn handle_one_connection(
    mut local: TcpStream,
    grpc_endpoint: &str,
    connection_url: &str,
    sk: &SigningKey,
    project_id: &str,
    board_id: &str,
    settings: Arc<RwLock<SettingsSnapshot>>,
    pool: Arc<ConnectionManager>,
    session_token_cli: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (headers_only, tail) = read_http_headers(&mut local).await?;
    let (host, port) = parse_connect_request(&headers_only)?;

    let (bypass, ep, session_tok, max_tunnels, wire_parsed) = {
        let snap = settings.read();
        let resolved_board =
            resolve_board_for_host(&host, &snap.data.routing, &snap.data.default_board);
        let use_board = if !board_id.is_empty() {
            board_id.to_string()
        } else {
            resolved_board
        };
        let (board_cfg, _) = board_config(&snap, &use_board);
        if !board_cfg.enabled {
            return Err(format!("board {use_board} is disabled in settings").into());
        }
        let wire_parsed = board_cfg.wire_subscription_uri.as_ref().and_then(|u| {
            let t = u.trim();
            if t.is_empty() {
                None
            } else {
                parse_subscription_uri(t).ok()
            }
        });
        let global_rules = snap.data.global.bypass.clone();
        let board_rules = board_cfg.bypass.clone();
        let g_m = BypassMatcher::from_rules(&global_rules).map_err(|e| e.to_string())?;
        let b_m = BypassMatcher::from_rules(&board_rules).map_err(|e| e.to_string())?;
        let bypass = g_m.matches_host(&host) || b_m.matches_host(&host);
        let ep = resolve_grpc_url(&board_cfg, grpc_endpoint, connection_url);
        let session_tok = session_token_for_board(&board_cfg, session_token_cli);
        let max_tunnels = board_cfg.max_concurrent_tunnels.or(Some(512));
        (bypass, ep, session_tok, max_tunnels, wire_parsed)
    };

    if bypass {
        return direct_connect_tunnel(local, &host, port, &headers_only, tail).await;
    }

    let sem = pool.semaphore_for(&ep, max_tunnels);
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("concurrency: {e}"))?;

    let mut client = DeployServiceClient::connect(ep.clone())
        .await
        .map_err(|e| format!("gRPC connect: {e}"))?;

    let metrics = Arc::new(TunnelMetrics::new());

    let (tx_req, rx_req) = mpsc::channel::<ProxyClientMsg>(64);
    let (open_host, open_port, wire_mode_o, wire_json_o) = if let Some(ref p) = wire_parsed {
        (
            "pirate.wire.local".to_string(),
            1u32,
            Some(p.mode.to_proto()),
            Some(
                p.params
                    .to_json_string()
                    .map_err(|e| format!("wire config: {e}"))?,
            ),
        )
    } else {
        (host.clone(), port as u32, None, None)
    };
    let open = ProxyClientMsg {
        body: Some(proxy_client_msg::Body::Open(ProxyOpen {
            host: open_host,
            port: open_port,
            session_token: session_tok.unwrap_or_default(),
            stream_correlation_id: String::new(),
            wire_mode: wire_mode_o,
            wire_config_json: wire_json_o,
        })),
    };
    tx_req.send(open).await?;

    let mut req = Request::new(ReceiverStream::new(rx_req));
    attach_auth_metadata(&mut req, sk, "ProxyTunnel", project_id, "")
        .map_err(|e| format!("auth metadata: {e}"))?;

    let mut outbound = client
        .proxy_tunnel(req)
        .await
        .map_err(|e| format!("ProxyTunnel: {e}"))?
        .into_inner();

    let first = outbound
        .message()
        .await
        .map_err(|e| format!("stream: {e}"))?
        .ok_or("empty ProxyTunnel response")?;

    match first.body {
        Some(proxy_server_msg::Body::OpenResult(r)) if r.ok => {}
        Some(proxy_server_msg::Body::OpenResult(r)) => {
            return Err(format!("upstream connect failed: {}", r.error).into());
        }
        _ => return Err("expected OpenResult from server".into()),
    }

    let tx_req = tx_req;
    if let Some(ref p) = wire_parsed {
        let chunk = wire_tunnel_first_chunk(
            p.mode,
            p.params.uuid.as_deref(),
            p.params.password.as_deref(),
            &host,
            port,
            &tail,
        )
        .map_err(|e| format!("wire handshake: {e}"))?;
        metrics.add_in(chunk.len() as u64);
        tx_req
            .send(ProxyClientMsg {
                body: Some(proxy_client_msg::Body::Data(chunk)),
            })
            .await?;
    } else if !tail.is_empty() {
        metrics.add_in(tail.len() as u64);
        tx_req
            .send(ProxyClientMsg {
                body: Some(proxy_client_msg::Body::Data(tail)),
            })
            .await?;
    }

    local
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    let (mut local_read, mut local_write) = local.into_split();
    let tx_req_clone = tx_req.clone();
    let metrics_in = metrics.clone();

    let to_server = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_CHUNK];
        loop {
            match tokio::time::timeout(Duration::from_secs(300), local_read.read(&mut buf)).await {
                Err(_) => break,
                Ok(Ok(0)) => {
                    let _ = tx_req_clone
                        .send(ProxyClientMsg {
                            body: Some(proxy_client_msg::Body::Fin(true)),
                        })
                        .await;
                    break;
                }
                Ok(Ok(n)) => {
                    if n > MAX_CHUNK {
                        break;
                    }
                    metrics_in.add_in(n as u64);
                    if tx_req_clone
                        .send(ProxyClientMsg {
                            body: Some(proxy_client_msg::Body::Data(buf[..n].to_vec())),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Err(_)) => break,
            }
        }
    });

    while let Some(item) = outbound.next().await {
        let msg = item.map_err(|e| e.to_string())?;
        match msg.body {
            Some(proxy_server_msg::Body::Data(data)) => {
                metrics.add_out(data.len() as u64);
                local_write.write_all(&data).await?;
            }
            Some(proxy_server_msg::Body::Eof(_)) => break,
            Some(proxy_server_msg::Body::Error(s)) => {
                return Err(format!("proxy error: {s}").into());
            }
            Some(proxy_server_msg::Body::OpenResult(_)) => {}
            None => {}
        }
    }

    to_server.abort();
    let _ = to_server.await;
    let _ = local_write.shutdown().await;
    metrics.finalize_wall_ms();
    Ok(())
}

async fn direct_connect_tunnel(
    mut local: TcpStream,
    host: &str,
    port: u16,
    _headers_only: &[u8],
    tail: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{host}:{port}");
    let mut remote = tokio::net::TcpStream::connect(&addr).await?;
    local
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    if !tail.is_empty() {
        remote.write_all(&tail).await?;
    }
    let (mut lr, mut lw) = local.into_split();
    let (mut rr, mut rw) = remote.into_split();
    let up = tokio::spawn(async move {
        let mut buf = [0u8; MAX_CHUNK];
        loop {
            match lr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if rw.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = rw.shutdown().await;
    });
    let mut buf = [0u8; MAX_CHUNK];
    loop {
        match rr.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if lw.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    up.abort();
    let _ = lw.shutdown().await;
    Ok(())
}
