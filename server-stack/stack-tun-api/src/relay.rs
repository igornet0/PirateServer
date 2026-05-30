//! Bidirectional relay between public TCP and the gRPC TunnelStream.

use crate::state::{signing_attach_tunnel, SharedState};
use crate::types::TunnelProfile;
use deploy_proto::stack_tun::stack_tun_service_client::StackTunServiceClient;
use deploy_proto::stack_tun::{
    tun_client_msg, tun_server_msg, TunAssigned, TunCliFin, TunClientMsg, TunFromConnector,
    TunFromListener, TunPull, TunServerMsg, TunSrvFin,
};
use ed25519_dalek::SigningKey;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const CHUNK: usize = 64 * 1024;

/// One long-lived connector session; outer loop reconnects on terminal errors.
pub async fn connector_run_session(
    st: &Arc<SharedState>,
    sk: &SigningKey,
    remote: String,
    profile: &TunnelProfile,
    listen_profile_id: &str,
) -> Result<(), String> {
    let endpoint = tonic::transport::Endpoint::from_shared(remote.trim().to_string())
        .map_err(|e| e.to_string())?
        .connect_timeout(Duration::from_secs(15))
        .tcp_nodelay(true);
    let chan = endpoint.connect().await.map_err(|e| e.to_string())?;
    let mut cli = StackTunServiceClient::new(chan);

    let (out_tx, out_rx) = mpsc::channel::<TunClientMsg>(256);
    let out_stream = ReceiverStream::new(out_rx);
    let mut req = tonic::Request::new(out_stream);
    signing_attach_tunnel(&mut req, sk, listen_profile_id.trim()).map_err(|e| e.to_string())?;

    let mut inbound = cli
        .tunnel_stream(req)
        .await
        .map_err(|e| e.to_string())?
        .into_inner();

    let first = inbound
        .message()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "empty server stream".to_string())?;
    match first.msg {
        Some(tun_server_msg::Msg::Ready(true)) => {}
        Some(tun_server_msg::Msg::Error(s)) => return Err(s),
        other => return Err(format!("unexpected first server message: {other:?}")),
    }

    loop {
        st.stats
            .connector_pulls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let wait = profile.pull_wait_ms.clamp(100, 120_000);
        out_tx
            .send(TunClientMsg {
                msg: Some(tun_client_msg::Msg::Pull(TunPull { wait_ms: wait })),
            })
            .await
            .map_err(|_| "connector outbound closed".to_string())?;

        let next = inbound.message().await.map_err(|e| e.to_string())?;
        let Some(sm) = next else {
            return Ok(());
        };

        match sm.msg {
            Some(tun_server_msg::Msg::Assigned(_)) => {}
            Some(tun_server_msg::Msg::Error(_)) => {
                // Listener dequeue timed out (no pending tcp); connector retries Pull quickly.
                continue;
            }
            other => return Err(format!("unexpected after pull: {other:?}")),
        }

        let upstream = format!("{}:{}", profile.target_host.trim(), profile.target_port);
        let tcp = TcpStream::connect(&upstream).await.map_err(|e| e.to_string())?;

        relay_connector_leg(&mut inbound, &out_tx, tcp)
            .await
            .map_err(|e| e.to_string())?;
    }
}

async fn relay_connector_leg(
    inbound: &mut tonic::Streaming<TunServerMsg>,
    out_tx: &mpsc::Sender<TunClientMsg>,
    tcp: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let mut buf = vec![0u8; CHUNK];
    loop {
        tokio::select! {
            m = inbound.message() => {
                let m = match m? {
                    Some(x) => x.msg,
                    None => break,
                };
                match m {
                    Some(tun_server_msg::Msg::FromListener(fl)) => {
                        tcp_w.write_all(&fl.chunk).await?;
                    }
                    Some(tun_server_msg::Msg::SrvFin(_)) => {
                        let _ = tcp_w.shutdown().await;
                        break;
                    }
                    Some(tun_server_msg::Msg::Error(err)) => return Err(err.into()),
                    Some(tun_server_msg::Msg::Ready(_) | tun_server_msg::Msg::Assigned(_)) => {
                        return Err("unexpected control message during relay".into());
                    }
                    None => break,
                }
            }
            n = tcp_r.read(&mut buf) => {
                let n = n?;
                if n == 0 {
                    let _ = out_tx
                        .send(TunClientMsg {
                            msg: Some(tun_client_msg::Msg::CliFin(TunCliFin {})),
                        })
                        .await;
                    break;
                }
                out_tx
                    .send(TunClientMsg {
                        msg: Some(tun_client_msg::Msg::FromConnector(TunFromConnector {
                            chunk: buf[..n].to_vec(),
                        })),
                    })
                    .await
                    .map_err(|_| "outbound closed")?;
            }
        }
    }
    Ok(())
}

/// Server-side: after `TunAssigned`, bridge public TCP ↔ connector-side stream frames.
pub async fn relay_listener_leg(
    inbound: &mut tonic::Streaming<TunClientMsg>,
    out_tx: &mpsc::Sender<Result<TunServerMsg, tonic::Status>>,
    tcp: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut tcp_r, mut tcp_w) = tcp.into_split();
    let mut buf = vec![0u8; CHUNK];
    loop {
        tokio::select! {
            m = inbound.message() => {
                let m = match m? {
                    Some(x) => x.msg,
                    None => break,
                };
                match m {
                    Some(tun_client_msg::Msg::FromConnector(fc)) => {
                        tcp_w.write_all(&fc.chunk).await?;
                    }
                    Some(tun_client_msg::Msg::CliFin(_)) => {
                        let _ = tcp_w.shutdown().await;
                        break;
                    }
                    Some(tun_client_msg::Msg::Pull(_)) => {
                        return Err("unexpected pull during relay".into());
                    }
                    None => break,
                }
            }
            n = tcp_r.read(&mut buf) => {
                let n = n?;
                if n == 0 {
                    out_tx
                        .send(Ok(TunServerMsg {
                            msg: Some(tun_server_msg::Msg::SrvFin(TunSrvFin {})),
                        }))
                        .await
                        .map_err(|_| "client disconnected")?;
                    break;
                }
                out_tx
                    .send(Ok(TunServerMsg {
                        msg: Some(tun_server_msg::Msg::FromListener(TunFromListener {
                            chunk: buf[..n].to_vec(),
                        })),
                    }))
                    .await
                    .map_err(|_| "client disconnected")?;
            }
        }
    }
    Ok(())
}

pub async fn tunnel_server_loop(
    st: Arc<SharedState>,
    prof: TunnelProfile,
    mut inbound: tonic::Streaming<TunClientMsg>,
    tx: mpsc::Sender<Result<TunServerMsg, tonic::Status>>,
) {
    loop {
        let msg = match inbound.message().await {
            Ok(Some(x)) => x,
            Ok(None) => break,
            Err(e) => {
                let _ = tx
                    .send(Err(tonic::Status::internal(format!("read client: {e}"))))
                    .await;
                break;
            }
        };
        match msg.msg {
            Some(tun_client_msg::Msg::Pull(p)) => {
                let q = { st.queues.lock().get(&prof.id).cloned() };
                let Some(q) = q else {
                    let _ = tx
                        .send(Err(tonic::Status::not_found("queue not ready")))
                        .await;
                    continue;
                };
                let wait = Duration::from_millis(p.wait_ms.clamp(50, 120_000));
                let Some(tcp) = q.dequeue(wait, &st.stats).await else {
                    let _ = tx
                        .send(Ok(TunServerMsg {
                            msg: Some(tun_server_msg::Msg::Error("pull_timeout".into())),
                        }))
                        .await;
                    continue;
                };
                if tx
                    .send(Ok(TunServerMsg {
                        msg: Some(tun_server_msg::Msg::Assigned(TunAssigned {})),
                    }))
                    .await
                    .is_err()
                {
                    break;
                }
                if let Err(e) = relay_listener_leg(&mut inbound, &tx, tcp).await {
                    st.stats
                        .relay_errors
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let _ = tx
                        .send(Err(tonic::Status::internal(format!("relay {e}"))))
                        .await;
                } else {
                    st.stats
                        .relay_completed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            _ => {
                let _ = tx
                    .send(Err(tonic::Status::invalid_argument(
                        "expected TunPull as first message each cycle",
                    )))
                    .await;
                break;
            }
        }
    }
}
