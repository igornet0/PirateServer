//! Local HTTP CONNECT proxy → gRPC `ProxyTunnel` on deploy-server.

use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::{proxy_client_msg, proxy_server_msg, ProxyClientMsg, ProxyOpen};
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;

const MAX_HEADER_READ: usize = 64 * 1024;
const MAX_CHUNK: usize = 256 * 1024;

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

/// Read until `\r\n\r\n` or limit. Returns header bytes (including final `\r\n\r\n`) and any bytes already read after that (e.g. TLS).
async fn read_http_headers(sock: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
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

/// Run local HTTP proxy: each CONNECT is tunneled over `ProxyTunnel` to `grpc_endpoint`.
pub async fn run_board(
    listen: &str,
    grpc_endpoint: &str,
    sk: &SigningKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(listen).await?;
    eprintln!("pirate board listening on {listen} → gRPC {grpc_endpoint}");
    loop {
        let (sock, peer) = listener.accept().await?;
        let ep = grpc_endpoint.to_string();
        let sk = sk.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_one_connection(sock, &ep, &sk).await {
                eprintln!("board {peer}: {e}");
            }
        });
    }
}

async fn handle_one_connection(
    mut local: TcpStream,
    grpc_endpoint: &str,
    sk: &SigningKey,
) -> Result<(), Box<dyn std::error::Error>> {
    let (headers_only, tail) = read_http_headers(&mut local).await?;

    let (host, port) = parse_connect_request(&headers_only)?;
    let mut client = DeployServiceClient::connect(grpc_endpoint.to_string())
        .await
        .map_err(|e| format!("gRPC connect: {e}"))?;

    let (tx_req, rx_req) = mpsc::channel::<ProxyClientMsg>(64);
    let open = ProxyClientMsg {
        body: Some(proxy_client_msg::Body::Open(ProxyOpen {
            host: host.clone(),
            port: port as u32,
        })),
    };
    tx_req.send(open).await?;

    let mut req = Request::new(ReceiverStream::new(rx_req));
    attach_auth_metadata(&mut req, sk, "ProxyTunnel", "default", "")
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

    local
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    let tx_req = tx_req;
    if !tail.is_empty() {
        tx_req
            .send(ProxyClientMsg {
                body: Some(proxy_client_msg::Body::Data(tail)),
            })
            .await?;
    }

    let (mut local_read, mut local_write) = local.into_split();
    let tx_req_clone = tx_req.clone();

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
    Ok(())
}
