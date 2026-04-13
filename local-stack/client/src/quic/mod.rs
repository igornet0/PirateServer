//! QUIC data-plane for proxy bytes (control plane stays gRPC).

mod dataplane;

pub use dataplane::relay_quic_data_plane;
