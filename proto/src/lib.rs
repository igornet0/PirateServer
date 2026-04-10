//! Generated protobuf + gRPC service definitions for `DeployService`.

pub mod deploy {
    tonic::include_proto!("deploy");
}

pub use deploy::deploy_service_client::DeployServiceClient;
pub use deploy::deploy_service_server::{DeployService, DeployServiceServer};
