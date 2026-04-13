//! TLS profile names for gRPC (`BoardConfig.tls_profile`).
//!
//! **Limitation:** `tonic::transport::ClientTlsConfig` does not expose raw `rustls::ClientConfig`
//! (cipher order, extension order, JA3). Profiles only vary **trust stores**: WebPKI roots vs
//! WebPKI + OS native roots (`compat`).

use crate::settings::BoardConfig;
use tonic::transport::ClientTlsConfig;

/// `modern` — WebPKI roots only. `compat` — WebPKI + native roots (broader trust).
pub fn client_tls_config(board: &BoardConfig) -> ClientTlsConfig {
    let mut c = ClientTlsConfig::new().with_webpki_roots();
    if board
        .tls_profile
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("compat"))
        .unwrap_or(false)
    {
        c = c.with_native_roots();
    }
    c
}
