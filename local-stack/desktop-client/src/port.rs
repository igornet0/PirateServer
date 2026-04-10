//! Bind first available TCP port on 127.0.0.1: 90, 9090, then 3000–9999.

use std::net::Ipv4Addr;
use tokio::net::TcpListener;

/// Privileged port 90 may fail without admin; we continue down the list.
pub async fn bind_first_available() -> Result<TcpListener, std::io::Error> {
    for p in std::iter::once(90u16)
        .chain(std::iter::once(9090u16))
        .chain(3000u16..=9999u16)
    {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, p)).await {
            Ok(l) => return Ok(l),
            Err(_) => continue,
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrNotAvailable,
        "no free TCP port in 90, 9090, or 3000–9999 on 127.0.0.1",
    ))
}
