//! Local HTTP CONNECT proxy → gRPC `ProxyTunnel` or direct TCP when bypass matches.

use crate::config::normalize_endpoint;
use crate::connection_manager::ConnectionManager;
use crate::metrics_collector::TunnelMetrics;
use crate::grpc_transport::{apply_stealth_metadata, stealth_jitter_before_rpc};
use crate::proxy_trace::{compact_grpc_endpoint_for_log, trace_log, ProxyTraceBuffer};
use crate::routing::resolve_board_for_host;
use crate::routing_rules::{tunnel_decision, TunnelDecision};
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

fn grpc_host_from_endpoint(ep: &str) -> String {
    let u = ep.trim();
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let end = rest
        .find(|c| c == ':' || c == '/')
        .unwrap_or(rest.len());
    if end == 0 {
        return "127.0.0.1".to_string();
    }
    rest[..end].to_string()
}

fn client_wants_quic(cfg: &BoardConfig) -> bool {
    match cfg.transport_mode.as_ref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref t) if t == "tcp" => false,
        _ => true,
    }
}

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
///
/// `trace`, when set, records one line per CONNECT outcome for the desktop log viewer.
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
    trace: Option<Arc<ProxyTraceBuffer>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(listen).await?;
    eprintln!("pirate board listening on {listen}");
    loop {
        let (sock, peer) = listener.accept().await?;
        let peer_addr = peer.to_string();
        let grpc_endpoint = grpc_endpoint.to_string();
        let connection_url = connection_url.to_string();
        let sk = sk.clone();
        let settings = settings.clone();
        let pool = pool.clone();
        let project_id = project_id.to_string();
        let board_id = board_id.to_string();
        let tok = session_token_cli.map(|s| s.to_string());
        let trace = trace.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_one_connection(
                sock,
                peer_addr,
                &grpc_endpoint,
                &connection_url,
                &sk,
                &project_id,
                &board_id,
                settings,
                pool,
                tok.as_deref(),
                trace,
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
    peer_addr: String,
    grpc_endpoint: &str,
    connection_url: &str,
    sk: &SigningKey,
    project_id: &str,
    board_id: &str,
    settings: Arc<RwLock<SettingsSnapshot>>,
    pool: Arc<ConnectionManager>,
    session_token_cli: Option<&str>,
    trace: Option<Arc<ProxyTraceBuffer>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (headers_only, tail) = read_http_headers(&mut local).await?;
    let (host, port) = parse_connect_request(&headers_only)?;

    let (board_cfg, ep, session_tok, max_tunnels, wire_parsed) = {
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
            trace_log(
                &trace,
                &peer_addr,
                format!("{host}:{port}"),
                "error",
                "board disabled",
                false,
                Some(format!("board {use_board}")),
            );
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
        let ep = resolve_grpc_url(&board_cfg, grpc_endpoint, connection_url);
        let session_tok = session_token_for_board(&board_cfg, session_token_cli);
        let max_tunnels = board_cfg.max_concurrent_tunnels.or_else(|| {
            std::env::var("PIRATE_MAX_CONCURRENT_TUNNELS_DEFAULT")
                .ok()
                .and_then(|s| s.parse().ok())
        }).or(Some(10));
        (board_cfg, ep, session_tok, max_tunnels, wire_parsed)
    };

    let decision = {
        let snap = settings.read();
        tunnel_decision(
            &host,
            &board_cfg,
            &snap.data.global,
            snap.compiled_default_rules.as_ref(),
        )?
    };

    match decision {
        TunnelDecision::Block => {
            trace_log(
                &trace,
                &peer_addr,
                format!("{host}:{port}"),
                "block",
                "rules -> 403 Forbidden",
                true,
                None,
            );
            local
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await?;
            return Ok(());
        }
        TunnelDecision::Direct => {
            return direct_connect_tunnel(
                local,
                &host,
                port,
                &headers_only,
                tail,
                &peer_addr,
                &trace,
            )
            .await;
        }
        TunnelDecision::Tunnel => {}
    }

    let target_label = format!("{host}:{port}");
    let ep_short = compact_grpc_endpoint_for_log(&ep);
    let route_tunnel = format!("gRPC({ep_short}) -> {target_label}");

    let sem = pool.semaphore_for(&ep, max_tunnels);
    let _permit = match sem.acquire_owned().await {
        Ok(p) => p,
        Err(e) => {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(format!("concurrency: {e}")),
            );
            return Err(format!("concurrency: {e}").into());
        }
    };

    stealth_jitter_before_rpc(&board_cfg).await;

    let channel = match pool.channel_for(&ep, &board_cfg) {
        Ok(c) => c,
        Err(e) => {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(format!("gRPC channel: {e}")),
            );
            return Err(format!("gRPC channel: {e}").into());
        }
    };
    let mut client = DeployServiceClient::new(channel);

    let metrics = Arc::new(TunnelMetrics::new());

    let (tx_req, rx_req) = mpsc::channel::<ProxyClientMsg>(64);
    let (open_host, open_port, wire_mode_o, wire_json_o) = if let Some(ref p) = wire_parsed {
        let wj = match p.params.to_json_string() {
            Ok(x) => x,
            Err(e) => {
                trace_log(
                    &trace,
                    &peer_addr,
                    &target_label,
                    "tunnel",
                    &route_tunnel,
                    false,
                    Some(format!("wire config: {e}")),
                );
                return Err(format!("wire config: {e}").into());
            }
        };
        (
            "pirate.wire.local".to_string(),
            1u32,
            Some(p.mode.to_proto()),
            Some(wj),
        )
    } else {
        (host.clone(), port as u32, None, None)
    };
    let tunnel_priority = board_cfg.tunnel_priority;
    let prefer_quic = client_wants_quic(&board_cfg);
    let open = ProxyClientMsg {
        body: Some(proxy_client_msg::Body::Open(ProxyOpen {
            host: open_host,
            port: open_port,
            session_token: session_tok.unwrap_or_default(),
            stream_correlation_id: String::new(),
            wire_mode: wire_mode_o,
            wire_config_json: wire_json_o,
            tunnel_priority,
            prefer_quic_data_plane: Some(prefer_quic),
        })),
    };
    if let Err(e) = tx_req.send(open).await {
        trace_log(
            &trace,
            &peer_addr,
            &target_label,
            "tunnel",
            &route_tunnel,
            false,
            Some(format!("send Open: {e}")),
        );
        return Err(e.into());
    }
    // Wire modes: server handshake (VLESS/Trojan/VMess) needs the first Data frame before it can
    // send OpenResult — send it before awaiting any server message (avoid deadlock).
    if let Some(ref p) = wire_parsed {
        let chunk = match wire_tunnel_first_chunk(
            p.mode,
            &p.params,
            &host,
            port,
            &tail,
        ) {
            Ok(c) => c,
            Err(e) => {
                trace_log(
                    &trace,
                    &peer_addr,
                    &target_label,
                    "tunnel",
                    &route_tunnel,
                    false,
                    Some(format!("wire handshake: {e}")),
                );
                return Err(format!("wire handshake: {e}").into());
            }
        };
        metrics.add_in(chunk.len() as u64);
        if let Err(e) = tx_req
            .send(ProxyClientMsg {
                body: Some(proxy_client_msg::Body::Data(chunk)),
            })
            .await
        {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(format!("send wire Data: {e}")),
            );
            return Err(e.into());
        }
    }

    let mut req = Request::new(ReceiverStream::new(rx_req));
    if let Err(e) = attach_auth_metadata(&mut req, sk, "ProxyTunnel", project_id, "") {
        let msg = e.to_string();
        trace_log(
            &trace,
            &peer_addr,
            &target_label,
            "tunnel",
            &route_tunnel,
            false,
            Some(format!("auth metadata: {msg}")),
        );
        return Err(format!("auth metadata: {e}").into());
    }
    if let Err(e) = apply_stealth_metadata(&board_cfg, &mut req) {
        trace_log(
            &trace,
            &peer_addr,
            &target_label,
            "tunnel",
            &route_tunnel,
            false,
            Some(e.clone()),
        );
        return Err(e.into());
    }

    let mut outbound = match client.proxy_tunnel(req).await {
        Ok(r) => r.into_inner(),
        Err(e) => {
            pool.invalidate_channel(&ep, &board_cfg);
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(e.to_string()),
            );
            return Err(format!("ProxyTunnel: {e}").into());
        }
    };

    let first = match outbound.message().await {
        Ok(Some(m)) => m,
        Ok(None) => {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some("empty ProxyTunnel response".into()),
            );
            return Err("empty ProxyTunnel response".into());
        }
        Err(e) => {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(e.to_string()),
            );
            return Err(format!("stream: {e}").into());
        }
    };

    let open_ok = match first.body {
        Some(proxy_server_msg::Body::OpenResult(r)) if r.ok => r,
        Some(proxy_server_msg::Body::OpenResult(r)) => {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(r.error.clone()),
            );
            return Err(format!("upstream connect failed: {}", r.error).into());
        }
        _ => {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some("expected OpenResult from server".into()),
            );
            return Err("expected OpenResult from server".into());
        }
    };

    if wire_parsed.is_none()
        && open_ok.quic_data_plane
        && prefer_quic
        && !open_ok.data_plane_ticket.is_empty()
    {
        let mut qh = open_ok.quic_host.clone();
        if qh.trim().is_empty() {
            qh = grpc_host_from_endpoint(&ep);
        }
        let qp = if open_ok.quic_port > 0 {
            open_ok.quic_port as u16
        } else {
            board_cfg.quic_port.unwrap_or(7844)
        };
        if let Err(e) = tx_req
            .send(ProxyClientMsg {
                body: Some(proxy_client_msg::Body::Fin(true)),
            })
            .await
        {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(format!("send Fin (quic handoff): {e}")),
            );
            return Err(e.into());
        }
        drop(tx_req);
        drop(outbound);
        let route_quic = format!("QUIC({qh}:{qp}) -> {target_label}");
        trace_log(
            &trace,
            &peer_addr,
            &target_label,
            "tunnel",
            &route_quic,
            true,
            None,
        );
        return crate::quic::relay_quic_data_plane(
            &qh,
            qp,
            &open_ok.data_plane_ticket,
            &host,
            port,
            board_cfg.quic_tls_insecure,
            tail,
            local,
        )
        .await;
    }

    trace_log(
        &trace,
        &peer_addr,
        &target_label,
        "tunnel",
        &route_tunnel,
        true,
        None,
    );

    let tx_req = tx_req;
    if wire_parsed.is_none() && !tail.is_empty() {
        metrics.add_in(tail.len() as u64);
        if let Err(e) = tx_req
            .send(ProxyClientMsg {
                body: Some(proxy_client_msg::Body::Data(tail)),
            })
            .await
        {
            trace_log(
                &trace,
                &peer_addr,
                &target_label,
                "tunnel",
                &route_tunnel,
                false,
                Some(format!("send tail: {e}")),
            );
            return Err(e.into());
        }
    }

    if let Err(e) = local
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
    {
        trace_log(
            &trace,
            &peer_addr,
            &target_label,
            "tunnel",
            &route_tunnel,
            false,
            Some(format!("HTTP 200 to client: {e}")),
        );
        return Err(e.into());
    }

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
        let msg = match item {
            Ok(m) => m,
            Err(e) => {
                trace_log(
                    &trace,
                    &peer_addr,
                    &target_label,
                    "tunnel",
                    &route_tunnel,
                    false,
                    Some(format!("stream: {e}")),
                );
                return Err(e.to_string().into());
            }
        };
        match msg.body {
            Some(proxy_server_msg::Body::Data(data)) => {
                metrics.add_out(data.len() as u64);
                if let Err(e) = local_write.write_all(&data).await {
                    trace_log(
                        &trace,
                        &peer_addr,
                        &target_label,
                        "tunnel",
                        &route_tunnel,
                        false,
                        Some(format!("write client: {e}")),
                    );
                    return Err(e.into());
                }
            }
            Some(proxy_server_msg::Body::Eof(_)) => break,
            Some(proxy_server_msg::Body::Error(s)) => {
                trace_log(
                    &trace,
                    &peer_addr,
                    &target_label,
                    "tunnel",
                    &route_tunnel,
                    false,
                    Some(format!("relay: {s}")),
                );
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
    peer_addr: &str,
    trace: &Option<Arc<ProxyTraceBuffer>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{host}:{port}");
    let target_label = addr.clone();
    let route = format!("direct -> {addr}");
    let mut remote = match tokio::net::TcpStream::connect(&addr).await {
        Ok(r) => r,
        Err(e) => {
            trace_log(
                trace,
                peer_addr,
                &target_label,
                "direct",
                &route,
                false,
                Some(e.to_string()),
            );
            return Err(e.into());
        }
    };
    trace_log(
        trace,
        peer_addr,
        &target_label,
        "direct",
        &route,
        true,
        None,
    );
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
