//! Listens for a single outbound TCP connection from `local-agent`, then proxies framed HTTP
//! requests to `CONTROL_API` (default `127.0.0.1:8080`).
//!
//! Wire format per round-trip: `u32` little-endian length + raw HTTP/1.x bytes.

use clap::Parser;
use std::io::ErrorKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

const MAX_FRAME: usize = 32 * 1024 * 1024;

#[derive(Parser, Debug)]
#[command(name = "tunnel-gateway", about = "Framed HTTP tunnel server for local-agent")]
struct Args {
    /// Address to listen on for the agent connection (one client at a time).
    #[arg(long, default_value = "[::]:8445")]
    listen: String,

    /// Upstream control-api (HTTP) address.
    #[arg(long, default_value = "127.0.0.1:8080")]
    control_api: String,
}

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

async fn proxy_one_roundtrip(
    agent: &mut TcpStream,
    upstream_addr: &str,
) -> std::io::Result<()> {
    let req = read_frame(agent).await?;
    if req.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "empty request frame",
        ));
    }

    let mut upstream = TcpStream::connect(upstream_addr).await?;
    upstream.write_all(&req).await?;
    let _ = upstream.shutdown().await;

    let mut resp = Vec::new();
    upstream.read_to_end(&mut resp).await?;
    write_frame(agent, &resp).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let listener = TcpListener::bind(&args.listen).await?;
    info!(listen = %args.listen, upstream = %args.control_api, "tunnel-gateway waiting for agent");

    loop {
        let (mut sock, peer) = listener.accept().await?;
        info!(%peer, "agent connected");
        let upstream = args.control_api.clone();
        tokio::spawn(async move {
            loop {
                match proxy_one_roundtrip(&mut sock, &upstream).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                        info!(%peer, "agent disconnected");
                        break;
                    }
                    Err(e) => {
                        warn!(%peer, error = %e, "proxy roundtrip failed");
                        break;
                    }
                }
            }
        });
    }
}
