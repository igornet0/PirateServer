//! Minimal Prometheus text exposition for deploy-server.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Default)]
pub struct ProxyTunnelMetrics {
    pub tunnels_open: AtomicU64,
    pub tunnels_total: AtomicU64,
    pub tunnel_errors: AtomicU64,
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    /// Currently waiting for tunnel admission slot.
    pub tunnel_waiters_current: AtomicU64,
    pub tunnel_wait_enqueue_total: AtomicU64,
    pub tunnel_wait_timeout_total: AtomicU64,
    /// QUIC data-plane bidirectional streams completed (raw relay).
    pub quic_stream_sessions_total: AtomicU64,
    /// QUIC stream failures (handshake/relay).
    pub quic_stream_errors_total: AtomicU64,
}

impl ProxyTunnelMetrics {
    pub fn render_prometheus(&self) -> String {
        let open = self.tunnels_open.load(Ordering::Relaxed);
        let total = self.tunnels_total.load(Ordering::Relaxed);
        let err = self.tunnel_errors.load(Ordering::Relaxed);
        let bi = self.bytes_in.load(Ordering::Relaxed);
        let bo = self.bytes_out.load(Ordering::Relaxed);
        let wcur = self.tunnel_waiters_current.load(Ordering::Relaxed);
        let weq = self.tunnel_wait_enqueue_total.load(Ordering::Relaxed);
        let wto = self.tunnel_wait_timeout_total.load(Ordering::Relaxed);
        let qss = self.quic_stream_sessions_total.load(Ordering::Relaxed);
        let qse = self.quic_stream_errors_total.load(Ordering::Relaxed);
        format!(
            "# HELP deploy_proxy_tunnels_open Currently open ProxyTunnel streams\n\
             # TYPE deploy_proxy_tunnels_open gauge\n\
             deploy_proxy_tunnels_open {open}\n\
             # HELP deploy_proxy_tunnels_total Total ProxyTunnel streams started\n\
             # TYPE deploy_proxy_tunnels_total counter\n\
             deploy_proxy_tunnels_total {total}\n\
             # HELP deploy_proxy_tunnel_errors_total ProxyTunnel stream errors\n\
             # TYPE deploy_proxy_tunnel_errors_total counter\n\
             deploy_proxy_tunnel_errors_total {err}\n\
             # HELP deploy_proxy_bytes_in_total Bytes client→upstream (proxy)\n\
             # TYPE deploy_proxy_bytes_in_total counter\n\
             deploy_proxy_bytes_in_total {bi}\n\
             # HELP deploy_proxy_bytes_out_total Bytes upstream→client (proxy)\n\
             # TYPE deploy_proxy_bytes_out_total counter\n\
             deploy_proxy_bytes_out_total {bo}\n\
             # HELP deploy_proxy_tunnel_waiters_current Waiters for admission slot\n\
             # TYPE deploy_proxy_tunnel_waiters_current gauge\n\
             deploy_proxy_tunnel_waiters_current {wcur}\n\
             # HELP deploy_proxy_tunnel_wait_enqueue_total Enqueued for admission\n\
             # TYPE deploy_proxy_tunnel_wait_enqueue_total counter\n\
             deploy_proxy_tunnel_wait_enqueue_total {weq}\n\
             # HELP deploy_proxy_tunnel_wait_timeout_total Admission wait timeouts\n\
             # TYPE deploy_proxy_tunnel_wait_timeout_total counter\n\
             deploy_proxy_tunnel_wait_timeout_total {wto}\n\
             # HELP deploy_quic_stream_sessions_total QUIC data-plane stream sessions completed\n\
             # TYPE deploy_quic_stream_sessions_total counter\n\
             deploy_quic_stream_sessions_total {qss}\n\
             # HELP deploy_quic_stream_errors_total QUIC data-plane stream errors\n\
             # TYPE deploy_quic_stream_errors_total counter\n\
             deploy_quic_stream_errors_total {qse}\n"
        )
    }
}

pub async fn serve_metrics_loop(addr: SocketAddr, m: Arc<ProxyTunnelMetrics>) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "metrics bind failed");
            return;
        }
    };
    tracing::info!(%addr, "Prometheus metrics HTTP");
    loop {
        let (mut sock, _) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(error = %e, "metrics accept");
                continue;
            }
        };
        let m = m.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let n = match sock.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = m.render_prometheus();
            let resp = if req.starts_with("GET /metrics") || req.starts_with("GET / ") {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
            };
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        });
    }
}
