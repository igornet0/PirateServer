//! Client-side first gRPC `data` chunk after `Open` for each wire mode.

use crate::params::{WireMode, WireParams};
use crate::shadowsocks::ss_client_first_chunk;
use crate::socks5::socks5_build_pipeline_connect;
use crate::trojan::{trojan_auth_line, trojan_build_address, TrojanAddr};
use crate::vless::{vless_build_request, VlessAddress};
use crate::vmess::vmess_client_header;
use crate::WireError;
use uuid::Uuid;

fn host_to_vless_addr(host: &str) -> VlessAddress {
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        VlessAddress::IpV4(ip.octets())
    } else if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        VlessAddress::IpV6(ip.octets())
    } else {
        VlessAddress::Domain(host.to_string())
    }
}

/// Bytes to send as the first `ProxyClientMsg::data` after successful `OpenResult`.
pub fn wire_tunnel_first_chunk(
    mode: WireMode,
    params: &WireParams,
    target_host: &str,
    target_port: u16,
    tcp_tail_after_connect: &[u8],
) -> Result<Vec<u8>, WireError> {
    let addr = host_to_vless_addr(target_host);
    match mode {
        WireMode::RawTcpRelay => Ok(tcp_tail_after_connect.to_vec()),
        WireMode::Vless => {
            let u = params
                .uuid
                .as_deref()
                .ok_or_else(|| WireError::Protocol("vless uuid".into()))?;
            let id = Uuid::parse_str(u.trim()).map_err(|e| WireError::Parse(e.to_string()))?;
            Ok(vless_build_request(
                &id,
                target_port,
                &addr,
                tcp_tail_after_connect,
            ))
        }
        WireMode::Trojan => {
            let pw = params
                .password
                .as_deref()
                .ok_or_else(|| WireError::Protocol("trojan password".into()))?;
            let mut out = trojan_auth_line(pw);
            let ta = TrojanAddr {
                host: target_host.to_string(),
                port: target_port,
            };
            out.extend_from_slice(&trojan_build_address(&ta)?);
            out.extend_from_slice(tcp_tail_after_connect);
            Ok(out)
        }
        WireMode::Vmess => {
            let u = params
                .uuid
                .as_deref()
                .ok_or_else(|| WireError::Protocol("vmess uuid".into()))?;
            let id = Uuid::parse_str(u.trim()).map_err(|e| WireError::Parse(e.to_string()))?;
            let mut h = vmess_client_header(&id, target_port, &addr)?;
            h.extend_from_slice(tcp_tail_after_connect);
            Ok(h)
        }
        WireMode::Shadowsocks => {
            let pw = params
                .password
                .as_deref()
                .ok_or_else(|| WireError::Protocol("shadowsocks password".into()))?;
            let method = params
                .method
                .as_deref()
                .ok_or_else(|| WireError::Protocol("shadowsocks method".into()))?;
            let ta = TrojanAddr {
                host: target_host.to_string(),
                port: target_port,
            };
            ss_client_first_chunk(method, pw, &ta, tcp_tail_after_connect)
        }
        WireMode::Socks5 => {
            let mut h = socks5_build_pipeline_connect(target_host, target_port)?;
            h.extend_from_slice(tcp_tail_after_connect);
            Ok(h)
        }
    }
}
