//! VLESS, Trojan, and VMess framing for Pirate `ProxyTunnel` (gRPC) — client/server pairs only.

mod client;
mod params;
mod trojan;
mod uri;
mod vless;
mod vmess;

pub use client::wire_tunnel_first_chunk;
pub use params::{WireError, WireMode, WireParams};
pub use trojan::{
    trojan_auth_line, trojan_parse_and_verify, trojan_server_handshake, TrojanHandshakeResult,
};
pub use uri::{parse_subscription_uri, ParsedSubscription};
pub use vless::{
    vless_build_request, vless_parse_request, VlessAddress, VlessParseResult, VLESS_VERSION,
};
pub use vmess::{
    vmess_aead_decode_chunk, vmess_aead_encode_chunk, vmess_check_replay, vmess_client_header,
    vmess_open_header_byte_len, vmess_server_open_header, VmessReplayCache, VMessHeaderOpen,
};
