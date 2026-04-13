//! Wire protocol (VLESS / Trojan / VMess / Shadowsocks / SOCKS5) over gRPC `ProxyTunnel` after handshake; post-handshake = raw TCP bytes.

use crate::metrics_http::ProxyTunnelMetrics;
use crate::proxy_session;
use crate::tunnel_flush::{flush_managed_tunnel_end, spawn_managed_checkpoint, ManagedTunnelCheckpoint};
use deploy_db::GrpcProxySessionRow;
use deploy_proto::deploy::{proxy_client_msg, proxy_server_msg, ProxyClientMsg, ProxyServerMsg};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tonic::Status;
use tracing::error;
use wire_protocol::{
    socks5_server_parse, ss_tcp_server_handshake, trojan_server_handshake, SsTcpHandshakeResult,
    Socks5ServerHandshake, TrojanHandshakeResult, vless_parse_request, vmess_check_replay,
    vmess_open_header_byte_len, vmess_server_open_header, VlessParseResult, VmessReplayCache,
    WireParams,
};

const MAX_PROXY_CHUNK: usize = 256 * 1024;

use deploy_proto::deploy::ProxyOpenResult;

#[inline]
fn proxy_open_result(ok: bool, err: impl AsRef<str>) -> ProxyOpenResult {
    ProxyOpenResult {
        ok,
        error: err.as_ref().to_string(),
        quic_data_plane: false,
        quic_host: String::new(),
        quic_port: 0,
        data_plane_ticket: Vec::new(),
    }
}

fn allowlist(host: &str) -> Result<(), Status> {
    let Ok(list) = std::env::var("DEPLOY_PROXY_ALLOWLIST") else {
        return Ok(());
    };
    let list = list.trim();
    if list.is_empty() || list == "*" {
        return Ok(());
    }
    let host_lc = host.to_ascii_lowercase();
    for part in list.split(',') {
        let p = part.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if host_lc == p || host_lc.ends_with(&format!(".{p}")) {
            return Ok(());
        }
    }
    Err(Status::permission_denied(
        "proxy target host not allowed by DEPLOY_PROXY_ALLOWLIST",
    ))
}

pub(crate) struct ActiveTracker {
    last: Option<Instant>,
    idle: Duration,
    accum_ms: u64,
}

impl ActiveTracker {
    pub(crate) fn new(idle_secs: u64) -> Self {
        Self {
            last: None,
            idle: Duration::from_secs(idle_secs.max(1)),
            accum_ms: 0,
        }
    }

    pub(crate) fn accum_ms(&self) -> u64 {
        self.accum_ms
    }

    fn bump(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last {
            let g = now.saturating_duration_since(prev);
            if g <= self.idle {
                self.accum_ms += g.as_millis() as u64;
            }
        }
        self.last = Some(now);
    }
}

/// Run VLESS: first gRPC `data` messages must contain a full VLESS request header + optional payload.
pub async fn run_vless_relay(
    mut inbound: tonic::Streaming<ProxyClientMsg>,
    tx: mpsc::Sender<Result<ProxyServerMsg, Status>>,
    params: WireParams,
    managed: Option<(GrpcProxySessionRow, proxy_session::StoredPolicy)>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    db_opt: Option<Arc<deploy_db::DbStore>>,
    client_pubkey_for_traffic: Option<String>,
    metrics: Arc<ProxyTunnelMetrics>,
    managed_checkpoint: Option<ManagedTunnelCheckpoint>,
) {
    let uuid_str = match params.uuid.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(
                        proxy_open_result(false, "vless: missing uuid in wire_config_json"),
                    )),
                }))
                .await;
            return;
        }
    };
    let expected = match uuid::Uuid::parse_str(uuid_str) {
        Ok(u) => u,
        Err(e) => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(proxy_open_result(
                        false,
                        format!("vless: bad uuid: {e}"),
                    ))),
                }))
                .await;
            return;
        }
    };

    let mut buf = Vec::<u8>::new();
    loop {
        match inbound.next().await {
            None => {
                let _ = tx
                    .send(Ok(ProxyServerMsg {
                        body: Some(proxy_server_msg::Body::OpenResult(
                            proxy_open_result(false, "vless: stream ended before header"),
                        )),
                    }))
                    .await;
                return;
            }
            Some(Err(e)) => {
                let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                return;
            }
            Some(Ok(msg)) => match msg.body {
                Some(proxy_client_msg::Body::Open(_)) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(false, "duplicate Open"),
                            )),
                        }))
                        .await;
                    return;
                }
                Some(proxy_client_msg::Body::Data(d)) => {
                    if d.len() > MAX_PROXY_CHUNK {
                        let _ = tx
                            .send(Ok(ProxyServerMsg {
                                body: Some(proxy_server_msg::Body::OpenResult(
                                    proxy_open_result(false, "proxy chunk too large"),
                                )),
                            }))
                            .await;
                        return;
                    }
                    buf.extend_from_slice(&d);
                    match vless_parse_request(&buf, Some(&expected)) {
                        VlessParseResult::NeedMore(_) => continue,
                        VlessParseResult::Invalid => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, "invalid vless header"),
                                    )),
                                }))
                                .await;
                            return;
                        }
                        VlessParseResult::Ok {
                            uuid_ok,
                            port,
                            addr,
                            payload,
                            payload_start,
                        } => {
                            if !uuid_ok {
                                let _ = tx
                                    .send(Ok(ProxyServerMsg {
                                        body: Some(proxy_server_msg::Body::OpenResult(
                                            proxy_open_result(false, "vless: uuid mismatch"),
                                        )),
                                    }))
                                    .await;
                                return;
                            }
                            let host = addr.host_string();
                            if let Err(e) = allowlist(&host) {
                                let _ = tx.send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, e.message().to_string()),
                                    )),
                                })).await;
                                return;
                            }
                            let upstream = format!("{host}:{port}");
                            let tcp = match tokio::time::timeout(
                                Duration::from_secs(30),
                                tokio::net::TcpStream::connect(&upstream),
                            )
                            .await
                            {
                                Ok(Ok(s)) => s,
                                Ok(Err(e)) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, e.to_string()),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                                Err(_) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, "connect timeout"),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                            };
                            if tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(true, String::new()),
                                    )),
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            let first_tail = buf[payload_start + payload.len()..].to_vec();
                            let (tcp_read, mut tcp_write) = tcp.into_split();
                            if !payload.is_empty() {
                                if tcp_write.write_all(payload).await.is_err() {
                                    return;
                                }
                                bytes_in.fetch_add(payload.len() as u64, Ordering::Relaxed);
                            }
                            run_raw_bridge(
                                inbound,
                                tx,
                                tcp_read,
                                tcp_write,
                                first_tail,
                                managed,
                                bytes_in,
                                bytes_out,
                                db_opt,
                                client_pubkey_for_traffic,
                                metrics,
                                managed_checkpoint,
                            )
                            .await;
                            return;
                        }
                    }
                }
                Some(proxy_client_msg::Body::Fin(_)) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(false, "vless: fin before header"),
                            )),
                        }))
                        .await;
                    return;
                }
                None => {}
            },
        }
    }
}

pub async fn run_trojan_relay(
    mut inbound: tonic::Streaming<ProxyClientMsg>,
    tx: mpsc::Sender<Result<ProxyServerMsg, Status>>,
    params: WireParams,
    managed: Option<(GrpcProxySessionRow, proxy_session::StoredPolicy)>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    db_opt: Option<Arc<deploy_db::DbStore>>,
    client_pubkey_for_traffic: Option<String>,
    metrics: Arc<ProxyTunnelMetrics>,
    managed_checkpoint: Option<ManagedTunnelCheckpoint>,
) {
    let password = match params.password.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(
                        proxy_open_result(false, "trojan: missing password"),
                    )),
                }))
                .await;
            return;
        }
    };
    let mut buf = Vec::<u8>::new();
    loop {
        match inbound.next().await {
            None => {
                let _ = tx
                    .send(Ok(ProxyServerMsg {
                        body: Some(proxy_server_msg::Body::OpenResult(
                            proxy_open_result(false, "trojan: stream ended before handshake"),
                        )),
                    }))
                    .await;
                return;
            }
            Some(Err(e)) => {
                let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                return;
            }
            Some(Ok(msg)) => match msg.body {
                Some(proxy_client_msg::Body::Open(_)) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(false, "duplicate Open"),
                            )),
                        }))
                        .await;
                    return;
                }
                Some(proxy_client_msg::Body::Data(d)) => {
                    buf.extend_from_slice(&d);
                    match trojan_server_handshake(&buf, password) {
                        TrojanHandshakeResult::NeedMore(_) => continue,
                        TrojanHandshakeResult::InvalidAuth => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, "trojan: auth failed"),
                                    )),
                                }))
                                .await;
                            return;
                        }
                        TrojanHandshakeResult::Ready {
                            addr,
                            payload_offset,
                        } => {
                            let host = addr.host.clone();
                            if let Err(e) = allowlist(&host) {
                                let _ = tx.send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, e.message().to_string()),
                                    )),
                                })).await;
                                return;
                            }
                            let upstream = format!("{}:{}", addr.host, addr.port);
                            let tcp = match tokio::time::timeout(
                                Duration::from_secs(30),
                                tokio::net::TcpStream::connect(&upstream),
                            )
                            .await
                            {
                                Ok(Ok(s)) => s,
                                Ok(Err(e)) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, e.to_string()),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                                Err(_) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, "connect timeout"),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                            };
                            if tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(true, String::new()),
                                    )),
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            let tail = buf[payload_offset..].to_vec();
                            let (tcp_read, tcp_write) = tcp.into_split();
                            run_raw_bridge(
                                inbound,
                                tx,
                                tcp_read,
                                tcp_write,
                                tail,
                                managed,
                                bytes_in,
                                bytes_out,
                                db_opt,
                                client_pubkey_for_traffic,
                                metrics,
                                managed_checkpoint,
                            )
                            .await;
                            return;
                        }
                    }
                }
                Some(proxy_client_msg::Body::Fin(_)) => return,
                None => {}
            },
        }
    }
}

static VMESS_REPLAY: std::sync::Mutex<Option<VmessReplayCache>> = std::sync::Mutex::new(None);

fn vmess_replay_cache() -> std::sync::MutexGuard<'static, Option<VmessReplayCache>> {
    VMESS_REPLAY.lock().unwrap()
}

pub async fn run_vmess_relay(
    mut inbound: tonic::Streaming<ProxyClientMsg>,
    tx: mpsc::Sender<Result<ProxyServerMsg, Status>>,
    params: WireParams,
    managed: Option<(GrpcProxySessionRow, proxy_session::StoredPolicy)>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    db_opt: Option<Arc<deploy_db::DbStore>>,
    client_pubkey_for_traffic: Option<String>,
    metrics: Arc<ProxyTunnelMetrics>,
    managed_checkpoint: Option<ManagedTunnelCheckpoint>,
) {
    let uuid_str = match params.uuid.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(
                        proxy_open_result(false, "vmess: missing uuid"),
                    )),
                }))
                .await;
            return;
        }
    };
    let uid = match uuid::Uuid::parse_str(uuid_str) {
        Ok(u) => u,
        Err(e) => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(proxy_open_result(
                        false,
                        format!("vmess: bad uuid: {e}"),
                    ))),
                }))
                .await;
            return;
        }
    };

    let mut buf = Vec::<u8>::new();
    loop {
        match inbound.next().await {
            None => return,
            Some(Err(e)) => {
                let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                return;
            }
            Some(Ok(msg)) => match msg.body {
                Some(proxy_client_msg::Body::Open(_)) => return,
                Some(proxy_client_msg::Body::Data(d)) => {
                    buf.extend_from_slice(&d);
                    let Some(hlen) = vmess_open_header_byte_len(&buf) else {
                        continue;
                    };
                    if buf.len() < hlen {
                        continue;
                    }
                    let nonce = &buf[..12];
                    let replay_ok = {
                        let mut g = vmess_replay_cache();
                        if g.is_none() {
                            *g = Some(VmessReplayCache::new(120, 50_000));
                        }
                        g.as_mut()
                            .map(|c| vmess_check_replay(c, nonce))
                            .unwrap_or(true)
                    };
                    if !replay_ok {
                        let _ = tx
                            .send(Ok(ProxyServerMsg {
                                body: Some(proxy_server_msg::Body::OpenResult(
                                    proxy_open_result(false, "vmess: replay"),
                                )),
                            }))
                            .await;
                        return;
                    }
                    let open = match vmess_server_open_header(&uid, &buf[..hlen]) {
                        Ok(Some(o)) => o,
                        Ok(None) => continue,
                        Err(e) => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, e.to_string()),
                                    )),
                                }))
                                .await;
                            return;
                        }
                    };
                    let host = open.addr.host_string();
                    if let Err(e) = allowlist(&host) {
                        let _ = tx.send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(false, e.message().to_string()),
                            )),
                        })).await;
                        return;
                    }
                    let upstream = format!("{host}:{}", open.port);
                    let tcp = match tokio::time::timeout(
                        Duration::from_secs(30),
                        tokio::net::TcpStream::connect(&upstream),
                    )
                    .await
                    {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, e.to_string()),
                                    )),
                                }))
                                .await;
                            return;
                        }
                        Err(_) => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, "connect timeout"),
                                    )),
                                }))
                                .await;
                            return;
                        }
                    };
                    if tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(true, String::new()),
                            )),
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let tail = buf[hlen..].to_vec();
                    let (tcp_read, tcp_write) = tcp.into_split();
                    run_raw_bridge(
                        inbound,
                        tx,
                        tcp_read,
                        tcp_write,
                        tail,
                        managed,
                        bytes_in,
                        bytes_out,
                        db_opt,
                        client_pubkey_for_traffic,
                        metrics,
                        managed_checkpoint,
                    )
                    .await;
                    return;
                }
                Some(proxy_client_msg::Body::Fin(_)) => return,
                None => {}
            },
        }
    }
}

pub async fn run_socks5_relay(
    mut inbound: tonic::Streaming<ProxyClientMsg>,
    tx: mpsc::Sender<Result<ProxyServerMsg, Status>>,
    params: WireParams,
    managed: Option<(GrpcProxySessionRow, proxy_session::StoredPolicy)>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    db_opt: Option<Arc<deploy_db::DbStore>>,
    client_pubkey_for_traffic: Option<String>,
    metrics: Arc<ProxyTunnelMetrics>,
    managed_checkpoint: Option<ManagedTunnelCheckpoint>,
) {
    let auth = params.username.is_some() && params.password.is_some();
    let user = params.username.as_deref();
    let pass = params.password.as_deref();
    let mut buf = Vec::<u8>::new();
    loop {
        match inbound.next().await {
            None => {
                let _ = tx
                    .send(Ok(ProxyServerMsg {
                        body: Some(proxy_server_msg::Body::OpenResult(
                            proxy_open_result(false, "socks5: stream ended before handshake"),
                        )),
                    }))
                    .await;
                return;
            }
            Some(Err(e)) => {
                let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                return;
            }
            Some(Ok(msg)) => match msg.body {
                Some(proxy_client_msg::Body::Open(_)) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(false, "duplicate Open"),
                            )),
                        }))
                        .await;
                    return;
                }
                Some(proxy_client_msg::Body::Data(d)) => {
                    if d.len() > MAX_PROXY_CHUNK {
                        let _ = tx
                            .send(Ok(ProxyServerMsg {
                                body: Some(proxy_server_msg::Body::OpenResult(
                                    proxy_open_result(false, "proxy chunk too large"),
                                )),
                            }))
                            .await;
                        return;
                    }
                    buf.extend_from_slice(&d);
                    match socks5_server_parse(&buf, auth, user, pass) {
                        Socks5ServerHandshake::NeedMore(_) => continue,
                        Socks5ServerHandshake::Invalid(m) => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, m),
                                    )),
                                }))
                                .await;
                            return;
                        }
                        Socks5ServerHandshake::Ready { target, consumed } => {
                            let host = target.host.clone();
                            if let Err(e) = allowlist(&host) {
                                let _ = tx.send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, e.message().to_string()),
                                    )),
                                })).await;
                                return;
                            }
                            let upstream = format!("{}:{}", target.host, target.port);
                            let tcp = match tokio::time::timeout(
                                Duration::from_secs(30),
                                tokio::net::TcpStream::connect(&upstream),
                            )
                            .await
                            {
                                Ok(Ok(s)) => s,
                                Ok(Err(e)) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, e.to_string()),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                                Err(_) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, "connect timeout"),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                            };
                            if tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(true, String::new()),
                                    )),
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            let tail = buf[consumed..].to_vec();
                            let (tcp_read, tcp_write) = tcp.into_split();
                            run_raw_bridge(
                                inbound,
                                tx,
                                tcp_read,
                                tcp_write,
                                tail,
                                managed,
                                bytes_in,
                                bytes_out,
                                db_opt,
                                client_pubkey_for_traffic,
                                metrics,
                                managed_checkpoint,
                            )
                            .await;
                            return;
                        }
                    }
                }
                Some(proxy_client_msg::Body::Fin(_)) => return,
                None => {}
            },
        }
    }
}

pub async fn run_shadowsocks_relay(
    mut inbound: tonic::Streaming<ProxyClientMsg>,
    tx: mpsc::Sender<Result<ProxyServerMsg, Status>>,
    params: WireParams,
    managed: Option<(GrpcProxySessionRow, proxy_session::StoredPolicy)>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    db_opt: Option<Arc<deploy_db::DbStore>>,
    client_pubkey_for_traffic: Option<String>,
    metrics: Arc<ProxyTunnelMetrics>,
    managed_checkpoint: Option<ManagedTunnelCheckpoint>,
) {
    let password = match params.password.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(
                        proxy_open_result(false, "shadowsocks: missing password"),
                    )),
                }))
                .await;
            return;
        }
    };
    let method = match params.method.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => {
            let _ = tx
                .send(Ok(ProxyServerMsg {
                    body: Some(proxy_server_msg::Body::OpenResult(
                        proxy_open_result(false, "shadowsocks: missing method"),
                    )),
                }))
                .await;
            return;
        }
    };
    let mut buf = Vec::<u8>::new();
    loop {
        match inbound.next().await {
            None => {
                let _ = tx
                    .send(Ok(ProxyServerMsg {
                        body: Some(proxy_server_msg::Body::OpenResult(
                            proxy_open_result(false, "shadowsocks: stream ended before handshake"),
                        )),
                    }))
                    .await;
                return;
            }
            Some(Err(e)) => {
                let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                return;
            }
            Some(Ok(msg)) => match msg.body {
                Some(proxy_client_msg::Body::Open(_)) => {
                    let _ = tx
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::OpenResult(
                                proxy_open_result(false, "duplicate Open"),
                            )),
                        }))
                        .await;
                    return;
                }
                Some(proxy_client_msg::Body::Data(d)) => {
                    if d.len() > MAX_PROXY_CHUNK {
                        let _ = tx
                            .send(Ok(ProxyServerMsg {
                                body: Some(proxy_server_msg::Body::OpenResult(
                                    proxy_open_result(false, "proxy chunk too large"),
                                )),
                            }))
                            .await;
                        return;
                    }
                    buf.extend_from_slice(&d);
                    match ss_tcp_server_handshake(&buf, method, password) {
                        SsTcpHandshakeResult::NeedMore(_) => continue,
                        SsTcpHandshakeResult::Invalid(m) => {
                            let _ = tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, format!("shadowsocks: {m}")),
                                    )),
                                }))
                                .await;
                            return;
                        }
                        SsTcpHandshakeResult::Ready {
                            addr,
                            consumed,
                            tail_after_addr,
                        } => {
                            let host = addr.host.clone();
                            if let Err(e) = allowlist(&host) {
                                let _ = tx.send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(false, e.message().to_string()),
                                    )),
                                })).await;
                                return;
                            }
                            let upstream = format!("{}:{}", addr.host, addr.port);
                            let tcp = match tokio::time::timeout(
                                Duration::from_secs(30),
                                tokio::net::TcpStream::connect(&upstream),
                            )
                            .await
                            {
                                Ok(Ok(s)) => s,
                                Ok(Err(e)) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, e.to_string()),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                                Err(_) => {
                                    let _ = tx
                                        .send(Ok(ProxyServerMsg {
                                            body: Some(proxy_server_msg::Body::OpenResult(
                                                proxy_open_result(false, "connect timeout"),
                                            )),
                                        }))
                                        .await;
                                    return;
                                }
                            };
                            if tx
                                .send(Ok(ProxyServerMsg {
                                    body: Some(proxy_server_msg::Body::OpenResult(
                                        proxy_open_result(true, String::new()),
                                    )),
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            let mut pending = tail_after_addr;
                            pending.extend_from_slice(&buf[consumed..]);
                            let (tcp_read, tcp_write) = tcp.into_split();
                            run_raw_bridge(
                                inbound,
                                tx,
                                tcp_read,
                                tcp_write,
                                pending,
                                managed,
                                bytes_in,
                                bytes_out,
                                db_opt,
                                client_pubkey_for_traffic,
                                metrics,
                                managed_checkpoint,
                            )
                            .await;
                            return;
                        }
                    }
                }
                Some(proxy_client_msg::Body::Fin(_)) => return,
                None => {}
            },
        }
    }
}

fn floor_to_utc_hour(dt: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    let ts = dt.timestamp();
    let hour_floor = ts - (ts.rem_euclid(3600));
    chrono::DateTime::from_timestamp(hour_floor, 0)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or(dt)
}

/// Post-handshake: gRPC `data` <-> TCP raw bytes (same as legacy `ProxyTunnel`).
async fn run_raw_bridge(
    mut inbound: tonic::Streaming<ProxyClientMsg>,
    tx: mpsc::Sender<Result<ProxyServerMsg, Status>>,
    mut tcp_read: tokio::net::tcp::OwnedReadHalf,
    mut tcp_write: tokio::net::tcp::OwnedWriteHalf,
    mut pending: Vec<u8>,
    managed: Option<(GrpcProxySessionRow, proxy_session::StoredPolicy)>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    db_opt: Option<Arc<deploy_db::DbStore>>,
    client_pubkey_for_traffic: Option<String>,
    metrics: Arc<ProxyTunnelMetrics>,
    managed_checkpoint: Option<ManagedTunnelCheckpoint>,
) {
    let managed_clone = managed.clone();
    let session_id_for_task = managed_clone
        .as_ref()
        .map(|(r, _)| r.session_id.clone());
    let pk_for_task = managed_clone
        .as_ref()
        .map(|(r, _)| r.client_pubkey_b64.clone());
    let policy_for_task = managed_clone.as_ref().map(|(_, p)| p.clone());
    let policy_for_t_in = policy_for_task.clone();
    let base_in = managed_clone
        .as_ref()
        .map(|(r, _)| r.bytes_in.max(0) as u64)
        .unwrap_or(0);
    let base_out = managed_clone
        .as_ref()
        .map(|(r, _)| r.bytes_out.max(0) as u64)
        .unwrap_or(0);
    let base_active_ms = managed_clone
        .as_ref()
        .map(|(r, _)| r.active_ms.max(0) as u64)
        .unwrap_or(0);
    let bin_count = bytes_in.clone();
    let bytes_out_in = bytes_out.clone();
    let idle_secs = managed_clone
        .as_ref()
        .map(|(_, p)| p.idle_timeout_sec())
        .unwrap_or(60);
    let active = Arc::new(std::sync::Mutex::new(ActiveTracker::new(idle_secs)));
    let active_in = active.clone();
    let active_out = active.clone();
    let active_end = active;

    let mut checkpoint_jh: Option<tokio::task::JoinHandle<()>> = None;
    let mut checkpoint_shut: Option<tokio::sync::watch::Sender<bool>> = None;
    if let Some(ref cp) = managed_checkpoint {
        let (jh, tx) = spawn_managed_checkpoint(
            cp.clone(),
            bytes_in.clone(),
            bytes_out.clone(),
            {
                let active_c = active_end.clone();
                move || active_c.lock().map(|a| a.accum_ms()).unwrap_or(0)
            },
        );
        checkpoint_jh = Some(jh);
        checkpoint_shut = Some(tx);
    }

    if !pending.is_empty() {
        bytes_in.fetch_add(pending.len() as u64, Ordering::Relaxed);
        if tcp_write.write_all(&pending).await.is_err() {
            return;
        }
        pending.clear();
    }

    let t_in = tokio::spawn(async move {
        while let Some(item) = inbound.next().await {
            let msg = match item {
                Ok(m) => m,
                Err(e) => {
                    let _ = tcp_write.shutdown().await;
                    return Err(Status::internal(e.to_string()));
                }
            };
            match msg.body {
                Some(proxy_client_msg::Body::Open(_)) => {
                    let _ = tcp_write.shutdown().await;
                    return Err(Status::invalid_argument("duplicate Open"));
                }
                Some(proxy_client_msg::Body::Data(data)) => {
                    if data.len() > MAX_PROXY_CHUNK {
                        let _ = tcp_write.shutdown().await;
                        return Err(Status::invalid_argument("proxy chunk too large"));
                    }
                    bin_count.fetch_add(data.len() as u64, Ordering::Relaxed);
                    let mut traffic_exceeded = false;
                    let mut budget_exceeded = false;
                    if let Ok(mut a) = active_in.lock() {
                        a.bump();
                        if let Some(ref pol) = policy_for_t_in {
                            let bi = base_in + bin_count.load(Ordering::Relaxed);
                            let bo = base_out + bytes_out_in.load(Ordering::Relaxed);
                            traffic_exceeded =
                                proxy_session::check_traffic_limits(pol, bi, bo).is_err();
                            if !traffic_exceeded {
                                budget_exceeded = proxy_session::active_time_budget_exceeded(
                                    pol,
                                    base_active_ms,
                                    a.accum_ms,
                                );
                            }
                        }
                    }
                    if traffic_exceeded {
                        let _ = tcp_write.shutdown().await;
                        return Err(Status::resource_exhausted("proxy session traffic limit"));
                    }
                    if budget_exceeded {
                        let _ = tcp_write.shutdown().await;
                        return Err(Status::resource_exhausted(
                            "proxy session active time budget exhausted",
                        ));
                    }
                    if tcp_write.write_all(&data).await.is_err() {
                        let _ = tcp_write.shutdown().await;
                        return Err(Status::internal("tcp write"));
                    }
                }
                Some(proxy_client_msg::Body::Fin(_)) => {
                    let _ = tcp_write.shutdown().await;
                    return Ok(());
                }
                None => {}
            }
        }
        let _ = tcp_write.shutdown().await;
        Ok(())
    });

    let mut buf = vec![0u8; MAX_PROXY_CHUNK];
    let tx_out = tx.clone();
    let bout_count = bytes_out.clone();
    let bytes_in_out = bytes_in.clone();
    let policy_out = policy_for_task.clone();
    let base_in_out = base_in;
    let base_out_out = base_out;
    let t_out = tokio::spawn(async move {
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => {
                    let _ = tx_out
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::Eof(true)),
                        }))
                        .await;
                    break;
                }
                Ok(n) => {
                    bout_count.fetch_add(n as u64, Ordering::Relaxed);
                    let mut budget_exceeded = false;
                    if let Ok(mut a) = active_out.lock() {
                        a.bump();
                        if let Some(ref pol) = policy_out {
                            let bi = base_in_out + bytes_in_out.load(Ordering::Relaxed);
                            let bo = base_out_out + bout_count.load(Ordering::Relaxed);
                            let _ = proxy_session::check_traffic_limits(pol, bi, bo);
                            budget_exceeded = proxy_session::active_time_budget_exceeded(
                                pol,
                                base_active_ms,
                                a.accum_ms,
                            );
                        }
                    }
                    if budget_exceeded {
                        let _ = tx_out
                            .send(Ok(ProxyServerMsg {
                                body: Some(proxy_server_msg::Body::Error(
                                    "proxy session active time budget exhausted".into(),
                                )),
                            }))
                            .await;
                        break;
                    }
                    let chunk = buf[..n].to_vec();
                    if tx_out
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::Data(chunk)),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx_out
                        .send(Ok(ProxyServerMsg {
                            body: Some(proxy_server_msg::Body::Error(e.to_string())),
                        }))
                        .await;
                    break;
                }
            }
        }
    });

    let _ = t_in.await;
    let _ = t_out.await;

    if let Some(tx) = checkpoint_shut {
        let _ = tx.send(true);
    }
    if let Some(jh) = checkpoint_jh {
        jh.abort();
    }

    let bi = bytes_in.load(Ordering::Relaxed);
    let bo = bytes_out.load(Ordering::Relaxed);

    let db_opt_for_hourly = db_opt.clone();
    let active_ms_u64 = active_end.lock().map(|a| a.accum_ms()).unwrap_or(0);

    if let Some(ref cp) = managed_checkpoint {
        if let Err(e) = flush_managed_tunnel_end(cp, bi, bo, active_ms_u64).await {
            error!(%e, "grpc proxy session final flush (wire bridge)");
        }
    } else if let (Some(db), Some(sid), Some(pk), Some(_pol)) = (
        db_opt,
        session_id_for_task,
        pk_for_task,
        policy_for_task,
    ) {
        let now = chrono::Utc::now();
        let _ = db
            .increment_grpc_proxy_session_traffic(
                &sid,
                &pk,
                bi,
                bo,
                active_ms_u64 as i64,
                now,
                Some(now),
            )
            .await;
    }

    metrics.bytes_in.fetch_add(bi, Ordering::Relaxed);
    metrics.bytes_out.fetch_add(bo, Ordering::Relaxed);

    let db_for_hourly = db_opt_for_hourly;
    if let (Some(db), Some(pk)) = (db_for_hourly, client_pubkey_for_traffic.clone()) {
        if bi > 0 || bo > 0 {
            let hour = floor_to_utc_hour(chrono::Utc::now());
            let db2 = db.clone();
            tokio::spawn(async move {
                if let Err(e) = db2.add_grpc_proxy_traffic_hourly(&pk, hour, bi, bo).await {
                    error!(%e, "grpc proxy traffic hourly");
                }
            });
        }
    }
}
