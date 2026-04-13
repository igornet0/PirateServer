//! QUIC listener: one bidirectional stream per logical CONNECT after auth ticket lookup.

use tracing::warn;
use wire_protocol::{encode_ack, StreamInitFrame};

use super::relay::{read_init_frame, run_raw_quic_relay};
use super::ticket::QuicTicketStore;

pub async fn handle_quic_bi_stream(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    store: QuicTicketStore,
) {
    let init_buf = match read_init_frame(&mut recv).await {
        Ok(b) => b,
        Err(e) => {
            let _ = send
                .write_all(&encode_ack(false, Some(&format!("read init: {e}"))))
                .await;
            let _ = send.finish();
            return;
        }
    };

    let frame = match StreamInitFrame::decode(&init_buf) {
        Ok(f) => f,
        Err(e) => {
            let _ = send
                .write_all(&encode_ack(false, Some(&format!("decode init: {e}"))))
                .await;
            let _ = send.finish();
            return;
        }
    };

    let Some(ctx) = store.take(&frame.ticket).await else {
        let _ = send
            .write_all(&encode_ack(
                false,
                Some("invalid or expired data-plane ticket"),
            ))
            .await;
        let _ = send.finish();
        return;
    };

    run_raw_quic_relay(send, recv, ctx, frame).await;
}

pub async fn run_quic_accept_loop(endpoint: quinn::Endpoint, store: QuicTicketStore) {
    while let Some(incoming) = endpoint.accept().await {
        let store = store.clone();
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "quic handshake failed");
                    return;
                }
            };
            loop {
                match conn.accept_bi().await {
                    Ok((send, recv)) => {
                        let store = store.clone();
                        tokio::spawn(async move {
                            handle_quic_bi_stream(send, recv, store).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });
    }
}
