//! Generated protobuf + gRPC service definitions for `DeployService`.

/// Cargo package version of this crate (linked into `pirate` / deploy clients).
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod deploy {
    tonic::include_proto!("deploy");
}

pub use deploy::deploy_service_client::DeployServiceClient;
pub use deploy::deploy_service_server::{DeployService, DeployServiceServer};
