//! TCP connection wrapping + async inserts into `grpc_session_events`.

use deploy_auth::META_PUBKEY;
use deploy_db::DbStore;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tonic::metadata::MetadataMap;
use tonic::transport::server::Connected;
use tonic::Request;
use tonic::transport::server::TcpConnectInfo;
use tracing::error;

/// Per-connection metadata inserted into each gRPC `Request` (tonic transport).
#[derive(Clone)]
pub struct AuditedConnectInfo {
    pub remote_addr: Option<SocketAddr>,
    #[allow(dead_code)]
    pub conn_id: u64,
    pub pubkey_slot: Arc<Mutex<Option<String>>>,
}

pub struct AuditedTcpStream {
    inner: TcpStream,
    hub: Arc<SessionAuditHub>,
    conn_id: u64,
    pubkey_slot: Arc<Mutex<Option<String>>>,
}

impl AuditedTcpStream {
    pub fn new(inner: TcpStream, hub: Arc<SessionAuditHub>) -> Self {
        let conn_id = hub.next_conn_id();
        let pubkey_slot = Arc::new(Mutex::new(None));
        let ip = inner
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        hub.spawn_insert(
            "tcp_open",
            None,
            &ip,
            "",
            "ok",
            &format!("conn_id={conn_id}"),
        );
        Self {
            inner,
            hub,
            conn_id,
            pubkey_slot,
        }
    }
}

impl Connected for AuditedTcpStream {
    type ConnectInfo = AuditedConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        AuditedConnectInfo {
            remote_addr: self.inner.peer_addr().ok(),
            conn_id: self.conn_id,
            pubkey_slot: self.pubkey_slot.clone(),
        }
    }
}

impl AsyncRead for AuditedTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for AuditedTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Drop for AuditedTcpStream {
    fn drop(&mut self) {
        let hub = self.hub.clone();
        let conn_id = self.conn_id;
        let ip = self
            .inner
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let pk = self.pubkey_slot.lock().clone();
        hub.spawn_insert(
            "tcp_close",
            pk.as_deref(),
            &ip,
            "",
            "ok",
            &format!("conn_id={conn_id}"),
        );
    }
}

pub struct SessionAuditHub {
    db: Option<Arc<DbStore>>,
    next_conn_id: AtomicU64,
}

impl SessionAuditHub {
    pub fn new(db: Option<Arc<DbStore>>) -> Arc<Self> {
        Arc::new(Self {
            db,
            next_conn_id: AtomicU64::new(1),
        })
    }

    fn next_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    fn spawn_insert(
        &self,
        kind: &str,
        client_pubkey_b64: Option<&str>,
        peer_ip: &str,
        grpc_method: &str,
        status: &str,
        detail: &str,
    ) {
        let Some(db) = self.db.clone() else {
            return;
        };
        let kind = kind.to_string();
        let peer_ip = peer_ip.to_string();
        let grpc_method = grpc_method.to_string();
        let status = status.to_string();
        let detail = detail.to_string();
        let pk = client_pubkey_b64.map(|s| s.to_string());
        tokio::spawn(async move {
            if let Err(e) = db
                .insert_grpc_session_event(
                    &kind,
                    pk.as_deref(),
                    &peer_ip,
                    &grpc_method,
                    &status,
                    &detail,
                )
                .await
            {
                error!(%e, kind = %kind, "grpc_session_events insert");
            }
        });
    }

    pub fn log_pair_outcome(
        &self,
        ok: bool,
        peer_ip: &str,
        client_pubkey_b64: Option<&str>,
        detail: &str,
    ) {
        let kind = if ok { "pair_ok" } else { "pair_denied" };
        let status = if ok { "ok" } else { "denied" };
        self.spawn_insert(kind, client_pubkey_b64, peer_ip, "Pair", status, detail);
    }
}

pub fn peer_ip_from_request<T>(req: &Request<T>) -> String {
    if let Some(a) = req.extensions().get::<AuditedConnectInfo>() {
        if let Some(addr) = a.remote_addr {
            return addr.to_string();
        }
    }
    req.extensions()
        .get::<TcpConnectInfo>()
        .and_then(|t| t.remote_addr())
        .map(|a| a.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// After successful `verify_*`, record client pubkey on this TCP connection (for `tcp_close`).
pub fn register_authenticated_client<T>(request: &Request<T>, meta: &MetadataMap) {
    let Some(pk) = meta.get(META_PUBKEY).and_then(|v| v.to_str().ok()) else {
        return;
    };
    if let Some(ci) = request.extensions().get::<AuditedConnectInfo>() {
        *ci.pubkey_slot.lock() = Some(pk.to_string());
    }
}
