//! Outbound tunnel to `tunnel-gateway`: local HTTP → framed bytes → server's control-api.
//!
//! Protocol matches [`tunnel-gateway`](../../../server-stack/tunnel-gateway): each message is
//! `u32` LE length + payload (raw HTTP/1.x request or response).

use std::io::ErrorKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_FRAME: usize = 32 * 1024 * 1024;
const MAX_HTTP: usize = 32 * 1024 * 1024;

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let n = u32::from_le_bytes(len_buf) as usize;
    if n > MAX_FRAME {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; n];
    if n > 0 {
        r.read_exact(&mut buf).await?;
    }
    Ok(buf)
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    let n = data.len() as u32;
    w.write_all(&n.to_le_bytes()).await?;
    w.write_all(data).await?;
    w.flush().await?;
    Ok(())
}

fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn content_length_prefix(headers: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(headers).ok()?;
    for line in s.lines() {
        let mut p = line.splitn(2, ':');
        let name = p.next()?.trim();
        let val = p.next()?.trim();
        if name.eq_ignore_ascii_case("content-length") {
            return val.parse().ok();
        }
    }
    None
}

/// Read one HTTP/1.x request from the stream (headers + body for known Content-Length).
async fn read_http_request(sock: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        if buf.len() > MAX_HTTP {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "HTTP request too large",
            ));
        }
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            if buf.is_empty() {
                return Err(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "connection closed before request",
                ));
            }
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(he) = header_end(&buf) {
            let cl = content_length_prefix(&buf[..he]).unwrap_or(0);
            if buf.len() >= he + cl {
                return Ok(buf[..he + cl].to_vec());
            }
        }
    }
    Ok(buf)
}

pub fn print_info() {
    println!("local-agent tunnel (HTTP over framed TCP)\n");
    println!("Run on the server host:");
    println!("  tunnel-gateway --listen [::]:8445 --control-api 127.0.0.1:8080");
    println!("On this PC (outbound only):");
    println!("  local-agent tunnel --server HOST:8445 --local 127.0.0.1:9999");
    println!("Then: curl -sS http://127.0.0.1:9999/api/v1/status");
    println!("\nSecurity: use VPN, SSH, or mTLS in front of tunnel-gateway in production.");
}

/// Connect outbound to `tunnel-gateway`, listen locally, proxy one HTTP request/response per connection.
pub async fn run_tunnel(
    server: &str,
    local_listen: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(local_listen).await?;
    tracing::info!(listen = %local_listen, "local-agent waiting for HTTP");
    loop {
        let (mut local, peer) = listener.accept().await?;
        tracing::info!(%peer, "local connection");
        let server = server.to_string();
        tokio::spawn(async move {
            let req = match read_http_request(&mut local).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "read local HTTP");
                    return;
                }
            };
            let mut remote = match TcpStream::connect(&server).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(error = %e, "connect tunnel server");
                    return;
                }
            };
            if let Err(e) = write_frame(&mut remote, &req).await {
                tracing::error!(error = %e, "write request frame");
                return;
            }
            let resp = match read_frame(&mut remote).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(error = %e, "read response frame");
                    return;
                }
            };
            if let Err(e) = local.write_all(&resp).await {
                tracing::warn!(error = %e, "write response to client");
            }
        });
    }
}
