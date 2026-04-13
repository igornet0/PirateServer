//! Raw TCP relay over a QUIC bidirectional stream (after init frame + ack).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::error;

use crate::metrics_http::ProxyTunnelMetrics;
use crate::proxy_session;
use crate::tunnel_flush::{flush_managed_tunnel_end, spawn_managed_checkpoint};
use wire_protocol::{
    encode_ack, StreamInitFrame, CMD_CONNECT, CMD_HEALTH_CHECK, ADDR_DOMAIN, ADDR_IPV4, ADDR_IPV6,
};

use super::context::QuicRawContext;

fn dest_matches(expected_host: &str, expected_port: u16, frame: &StreamInitFrame) -> bool {
    if frame.port != expected_port {
        return false;
    }
    match frame.addr_type {
        ADDR_DOMAIN => std::str::from_utf8(&frame.addr)
            .ok()
            .map(|s| s.eq_ignore_ascii_case(expected_host))
            .unwrap_or(false),
        ADDR_IPV4 => {
            if frame.addr.len() != 4 {
                return false;
            }
            let ip = format!("{}.{}.{}.{}", frame.addr[0], frame.addr[1], frame.addr[2], frame.addr[3]);
            ip == expected_host
        }
        ADDR_IPV6 => std::str::from_utf8(&frame.addr)
            .ok()
            .map(|s| s == expected_host)
            .unwrap_or(false),
        _ => false,
    }
}

pub(crate) async fn read_init_frame(recv: &mut quinn::RecvStream) -> Result<Vec<u8>, std::io::Error> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = match recv.read(&mut tmp).await {
            Ok(None) => {
                if buf.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "empty stream",
                    ));
                }
                break;
            }
            Ok(Some(n)) => n,
            Err(e) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ));
            }
        };
        if n == 0 {
            continue;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 65536 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "init frame too large",
            ));
        }
        if let Some(wl) = StreamInitFrame::wire_len(&buf) {
            if buf.len() >= wl {
                return Ok(buf[..wl].to_vec());
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "short init",
    ))
}

/// Raw relay after init frame was read and ticket validated against [`QuicRawContext`].
pub async fn run_raw_quic_relay(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    ctx: QuicRawContext,
    frame: StreamInitFrame,
) {
    let metrics = ctx.metrics.clone();

    if frame.command == CMD_HEALTH_CHECK {
        let _ = send.write_all(&encode_ack(true, None)).await;
        let _ = send.finish();
        finish_quic_raw(ctx, metrics.clone()).await;
        return;
    }

    if frame.command != CMD_CONNECT {
        let _ = send
            .write_all(&encode_ack(
                false,
                Some("unsupported command (use CONNECT or HEALTH_CHECK)"),
            ))
            .await;
        let _ = send.finish();
        metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
        let _ = ctx.completion.send(());
        return;
    }

    if !dest_matches(&ctx.expected_host, ctx.expected_port, &frame) {
        let _ = send
            .write_all(&encode_ack(
                false,
                Some("destination mismatch vs ProxyOpen"),
            ))
            .await;
        let _ = send.finish();
        metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
        let _ = ctx.completion.send(());
        return;
    }

    let addr = format!("{}:{}", ctx.expected_host, ctx.expected_port);
    let tcp = match tokio::time::timeout(
        Duration::from_secs(30),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
            let _ = send.write_all(&encode_ack(false, Some(&e.to_string()))).await;
            let _ = send.finish();
            let _ = ctx.completion.send(());
            return;
        }
        Err(_) => {
            metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
            let _ = send
                .write_all(&encode_ack(false, Some("connect timeout")))
                .await;
            let _ = send.finish();
            let _ = ctx.completion.send(());
            return;
        }
    };

    if send.write_all(&encode_ack(true, None)).await.is_err() {
        metrics.tunnel_errors.fetch_add(1, Ordering::Relaxed);
        let _ = ctx.completion.send(());
        return;
    }

    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let bytes_in = ctx.bytes_in.clone();
    let bytes_out = ctx.bytes_out.clone();
    let policy_t_in = ctx.policy_for_task.clone();
    let policy_t_out = ctx.policy_for_task.clone();
    let base_in = ctx.base_in;
    let base_out = ctx.base_out;
    let base_active_ms = ctx.base_active_ms;
    let active_in = ctx.active.clone();
    let active_out = ctx.active.clone();
    let active_end = ctx.active.clone();

    let mut checkpoint_jh: Option<tokio::task::JoinHandle<()>> = None;
    let mut checkpoint_shut: Option<tokio::sync::watch::Sender<bool>> = None;
    if let Some(ref cp) = ctx.managed_checkpoint {
        let cp = cp.clone();
        let (jh, tx) = spawn_managed_checkpoint(
            cp,
            bytes_in.clone(),
            bytes_out.clone(),
            {
                let active_c = active_end.clone();
                move || active_c.lock().map(|a| a.accum_ms).unwrap_or(0)
            },
        );
        checkpoint_jh = Some(jh);
        checkpoint_shut = Some(tx);
    }

    const MAX_PROXY_CHUNK: usize = 256 * 1024;
    let mut buf_up = vec![0u8; MAX_PROXY_CHUNK];
    let mut buf_down = vec![0u8; MAX_PROXY_CHUNK];

    let bin_count = bytes_in.clone();
    let bout_count = bytes_out.clone();
    let bytes_out_in = bytes_out.clone();
    let bytes_in_for_out = bytes_in.clone();

    let t_up = tokio::spawn(async move {
        loop {
            let n = match recv.read(&mut buf_up).await {
                Ok(None) | Ok(Some(0)) => break,
                Ok(Some(n)) => n,
                Err(_) => break,
            };
            if n > MAX_PROXY_CHUNK {
                break;
            }
            bin_count.fetch_add(n as u64, Ordering::Relaxed);
            let mut traffic_exceeded = false;
            let mut budget_exceeded = false;
            if let Ok(mut a) = active_in.lock() {
                a.bump();
                if let Some(ref pol) = policy_t_in {
                    let bi = base_in + bin_count.load(Ordering::Relaxed);
                    let bo = base_out + bytes_out_in.load(Ordering::Relaxed);
                    traffic_exceeded = proxy_session::check_traffic_limits(pol, bi, bo).is_err();
                    if !traffic_exceeded {
                        budget_exceeded = proxy_session::active_time_budget_exceeded(
                            pol,
                            base_active_ms,
                            a.accum_ms,
                        );
                    }
                }
            }
            if traffic_exceeded || budget_exceeded {
                break;
            }
            if tcp_write.write_all(&buf_up[..n]).await.is_err() {
                break;
            }
        }
        let _ = tcp_write.shutdown().await;
    });

    let t_down = tokio::spawn(async move {
        loop {
            match tcp_read.read(&mut buf_down).await {
                Ok(0) => break,
                Ok(n) => {
                    bout_count.fetch_add(n as u64, Ordering::Relaxed);
                    let mut budget_exceeded = false;
                    if let Ok(mut a) = active_out.lock() {
                        a.bump();
                        if let Some(ref pol) = policy_t_out {
                            let bi = base_in + bytes_in_for_out.load(Ordering::Relaxed);
                            let bo = base_out + bout_count.load(Ordering::Relaxed);
                            let _ = proxy_session::check_traffic_limits(pol, bi, bo);
                            budget_exceeded = proxy_session::active_time_budget_exceeded(
                                pol,
                                base_active_ms,
                                a.accum_ms,
                            );
                        }
                    }
                    if budget_exceeded {
                        break;
                    }
                    if send.write_all(&buf_down[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = send.finish();
    });

    let _ = tokio::join!(t_up, t_down);

    if let Some(tx) = checkpoint_shut {
        let _ = tx.send(true);
    }
    if let Some(jh) = checkpoint_jh {
        jh.abort();
    }

    finish_quic_raw(ctx, metrics.clone()).await;
}

async fn finish_quic_raw(ctx: QuicRawContext, metrics: Arc<ProxyTunnelMetrics>) {
    let bi = ctx.bytes_in.load(Ordering::Relaxed);
    let bo = ctx.bytes_out.load(Ordering::Relaxed);
    let active_ms_u64 = ctx
        .active
        .lock()
        .map(|a| a.accum_ms)
        .unwrap_or(0);

    if let Some(ref cp) = ctx.managed_checkpoint {
        if let Err(e) = flush_managed_tunnel_end(cp, bi, bo, active_ms_u64).await {
            error!(%e, "grpc proxy session final flush (quic tunnel)");
        }
    } else if let (Some(db), Some(sid), Some(pk), Some(_pol)) = (
        ctx.db_opt.clone(),
        ctx.session_id_for_task,
        ctx.pk_for_task,
        ctx.policy_for_task,
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
    metrics
        .quic_stream_sessions_total
        .fetch_add(1, Ordering::Relaxed);

    let db_for_hourly = ctx.db_opt.clone();
    if let (Some(db), Some(pk)) = (db_for_hourly, ctx.client_pubkey_for_traffic.clone()) {
        if bi > 0 || bo > 0 {
            let hour = crate::deploy_service::floor_to_utc_hour(chrono::Utc::now());
            let db2 = db.clone();
            tokio::spawn(async move {
                if let Err(e) = db2.add_grpc_proxy_traffic_hourly(&pk, hour, bi, bo).await {
                    error!(%e, "grpc proxy traffic hourly (quic)");
                }
            });
        }
    }

    let _ = ctx.completion.send(());
}
