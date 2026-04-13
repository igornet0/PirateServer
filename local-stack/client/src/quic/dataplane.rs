//! Connect to server QUIC listener and relay one CONNECT over a bidirectional stream.

use std::net::SocketAddr;
use std::sync::Arc;

use quinn::Endpoint;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::DigitallySignedStruct;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wire_protocol::{StreamInitFrame, ADDR_DOMAIN, CMD_CONNECT};

#[derive(Debug)]
struct SkipServerCerts;

impl ServerCertVerifier for SkipServerCerts {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

fn quic_client_config(insecure: bool) -> Result<quinn::ClientConfig, Box<dyn std::error::Error>> {
    let tls = if insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerCerts))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    let mut tls = tls;
    tls.alpn_protocols = vec![b"pirate-quic".to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(Arc::new(transport));
    Ok(cfg)
}

async fn read_ack(recv: &mut quinn::RecvStream) -> Result<(), Box<dyn std::error::Error>> {
    let mut b0 = [0u8; 1];
    recv.read_exact(&mut b0).await?;
    if b0[0] == wire_protocol::ACK_OK {
        return Ok(());
    }
    if b0[0] == wire_protocol::ACK_ERR {
        let mut lenb = [0u8; 2];
        recv.read_exact(&mut lenb).await?;
        let len = u16::from_be_bytes(lenb) as usize;
        let mut msg = vec![0u8; len];
        recv.read_exact(&mut msg).await?;
        let s = String::from_utf8_lossy(&msg);
        return Err(format!("upstream: {s}").into());
    }
    Err("bad QUIC ack".into())
}

/// Relay one CONNECT after gRPC `OpenResult` advertised QUIC data-plane.
/// Sends `HTTP/1.1 200 Connection Established` after QUIC handshake succeeds.
/// `tail` is forwarded as the first bytes on the QUIC stream (same role as first gRPC `Data` after open).
pub async fn relay_quic_data_plane(
    quic_host: &str,
    quic_port: u16,
    ticket: &[u8],
    target_host: &str,
    target_port: u16,
    insecure_tls: bool,
    tail: Vec<u8>,
    mut local: tokio::net::TcpStream,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{quic_host}:{quic_port}")
        .parse()
        .map_err(|e| format!("bad QUIC address: {e}"))?;
    let cfg = quic_client_config(insecure_tls)?;
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(cfg);

    let connecting = endpoint.connect(addr, quic_host)?;
    let conn = connecting.await?;

    let (mut send, mut recv) = conn.open_bi().await?;

    let frame = StreamInitFrame {
        command: CMD_CONNECT,
        ticket: ticket.to_vec(),
        addr_type: ADDR_DOMAIN,
        addr: target_host.as_bytes().to_vec(),
        port: target_port,
    };
    let init = frame.encode()?;
    send.write_all(&init).await?;

    read_ack(&mut recv).await?;

    local
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    if !tail.is_empty() {
        send.write_all(&tail).await?;
    }

    let (mut lr, mut lw) = local.into_split();
    let up = tokio::spawn(async move {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match lr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if send.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = send.finish();
    });

    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match recv.read(&mut buf).await {
            Ok(None) => break,
            Ok(Some(0)) => continue,
            Ok(Some(n)) => {
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
