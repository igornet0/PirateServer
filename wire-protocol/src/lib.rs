//! VLESS, Trojan, and VMess framing for Pirate `ProxyTunnel` (gRPC) — client/server pairs only.

mod client;
mod params;
mod shadowsocks;
mod socks5;
mod trojan;
mod uri;
mod vless;
mod vmess;
mod quic_dataplane;

pub use client::wire_tunnel_first_chunk;
pub use params::{WireError, WireMode, WireParams};
pub use shadowsocks::{ss_client_first_chunk, ss_tcp_server_handshake, SsTcpHandshakeResult};
pub use socks5::{socks5_build_pipeline_connect, socks5_server_parse, Socks5ServerHandshake};
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
pub use quic_dataplane::{
    decode_ack, encode_ack, StreamInitFrame, QuicDataPlaneError, AckError, ADDR_DOMAIN, ADDR_IPV4,
    ADDR_IPV6, CMD_CONNECT, CMD_HEALTH_CHECK, CMD_UDP_ASSOCIATE, FRAME_VERSION, MAGIC, ACK_OK,
    ACK_ERR,
};
