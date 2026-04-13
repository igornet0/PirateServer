//! QUIC data-plane (UDP) for proxy bytes; control-plane stays on gRPC.

pub mod context;
pub mod relay;
pub mod server;
pub mod ticket;

use std::net::SocketAddr;
use std::sync::Arc;

pub use server::run_quic_accept_loop;
pub use ticket::QuicTicketStore;

/// Shared state for issuing QUIC tickets from `ProxyTunnel` (gRPC).
#[derive(Clone)]
pub struct QuicDataplaneState {
    pub ticket_store: QuicTicketStore,
    pub public_host: String,
    pub public_port: u16,
}

/// When unset or non-false, QUIC data-plane is allowed (if listener starts).
pub fn dataplane_enabled() -> bool {
    std::env::var("DEPLOY_QUIC_DATAPLANE")
        .map(|v| {
            let t = v.trim();
            !(t == "0" || t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("off"))
        })
        .unwrap_or(true)
}

pub fn ticket_ttl() -> std::time::Duration {
    std::env::var("DEPLOY_QUIC_TICKET_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or_else(|| std::time::Duration::from_secs(120))
}

/// Build TLS + QUIC server config (TLS 1.3 via rustls).
pub fn build_server_config(
    cert_pem_path: Option<&std::path::Path>,
    key_pem_path: Option<&std::path::Path>,
) -> Result<quinn::ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let (certs, key) = if let (Some(c), Some(k)) = (cert_pem_path, key_pem_path) {
        load_certs_and_key(c, k)?
    } else {
        generate_self_signed()?
    };

    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    tls.alpn_protocols = vec![b"pirate-quic".to_vec()];
    tls.max_early_data_size = 0;

    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)?;
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(quinn::VarInt::from_u32(10_000));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));

    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    cfg.transport_config(Arc::new(transport));
    Ok(cfg)
}

fn load_certs_and_key(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<(Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>), Box<dyn std::error::Error + Send + Sync>>
{
    let cert_file = std::fs::File::open(cert_path)?;
    let mut cert_reader = std::io::BufReader::new(cert_file);
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    let key_file = std::fs::File::open(key_path)?;
    let mut key_reader = std::io::BufReader::new(key_file);
    let Some(key) = rustls_pemfile::private_key(&mut key_reader)? else {
        return Err("no private key in PEM".into());
    };
    Ok((certs, key))
}

fn generate_self_signed(
) -> Result<(Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>), Box<dyn std::error::Error + Send + Sync>>
{
    let certified = rcgen::generate_simple_self_signed(vec!["pirate-quic.local".into()])?;
    let cert_der = certified.cert.der().clone();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der());
    Ok((
        vec![cert_der],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key),
    ))
}

pub fn start_quic_listener(
    bind: SocketAddr,
    cert_pem: Option<&std::path::Path>,
    key_pem: Option<&std::path::Path>,
) -> Result<(quinn::Endpoint, QuicTicketStore), Box<dyn std::error::Error + Send + Sync>> {
    let server_config = build_server_config(cert_pem, key_pem)?;
    let endpoint = quinn::Endpoint::server(server_config, bind)?;
    let store = QuicTicketStore::new(ticket_ttl());
    Ok((endpoint, store))
}
