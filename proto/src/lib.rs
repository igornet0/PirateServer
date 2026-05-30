//! Generated protobuf + gRPC service definitions (`DeployService`, `StackTunService`, …).

/// Cargo package version of this crate (linked into `pirate` / deploy clients).
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod deploy {
    tonic::include_proto!("deploy");
}

pub mod stack_tun {
    tonic::include_proto!("stack_tun");
}

pub use deploy::deploy_service_client::DeployServiceClient;
pub use deploy::deploy_service_server::{DeployService, DeployServiceServer};

pub use stack_tun::stack_tun_service_client::StackTunServiceClient;
pub use stack_tun::stack_tun_service_server::{StackTunService, StackTunServiceServer};
