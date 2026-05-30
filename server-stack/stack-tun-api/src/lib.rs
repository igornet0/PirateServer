//! Library surface for integration tests (`stack-tun-api` binary is the primary artifact).

pub mod grpc_svc;
pub mod http_api;
pub mod queue;
pub mod relay;
pub mod request_bus;
pub mod state;
pub mod store;
pub mod task_queue;
pub mod types;

use crate::grpc_svc::TunGrpc;
use crate::state::SharedState;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tonic::transport::Server;

#[derive(Debug, Parser)]
#[command(name = "stack-tun-api")]
pub struct StackTunArgs {
    #[arg(long, env = "STACK_TUN_STATE_DIR", default_value = "/var/lib/pirate/stack-tun-api")]
    pub state_dir: PathBuf,

    #[arg(long, env = "STACK_TUN_HTTP_BIND", default_value = "127.0.0.1:9380")]
    pub http_bind: String,

    #[arg(long, env = "STACK_TUN_GRPC_BIND", default_value = "127.0.0.1:9381")]
    pub grpc_bind: String,

    #[arg(long, env = "STACK_TUN_AUTHORIZED_PEERS_PATH")]
    pub authorized_peers_path: Option<PathBuf>,

    #[arg(long, env = "STACK_TUN_IDENTITY_PATH")]
    pub identity_path: Option<PathBuf>,

    #[arg(long, env = "STACK_TUN_REST_BEARER")]
    pub rest_bearer: Option<String>,

    /// Dev only: disables gRPC metadata signature checks (dangerous).
    #[arg(long, env = "STACK_TUN_ALLOW_UNAUTHENTICATED", default_value_t = false)]
    pub allow_unauthenticated: bool,
}

pub async fn run(args: StackTunArgs) -> Result<(), String> {
    let http_addr: SocketAddr = args
        .http_bind
        .trim()
        .parse()
        .map_err(|e| format!("http bind {e}"))?;
    let grpc_addr: SocketAddr = args
        .grpc_bind
        .trim()
        .parse()
        .map_err(|e| format!("grpc bind {e}"))?;

    let state = SharedState::bootstrap(
        args.state_dir,
        args.authorized_peers_path,
        args.rest_bearer.clone(),
        args.identity_path,
        args.allow_unauthenticated,
    )
    .await?;

    let router = crate::http_api::router(Arc::clone(&state));
    let http_listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .map_err(|e| format!("http listen {e}"))?;

    let grpc_svc = TunGrpc::new(Arc::clone(&state));

    tracing::info!(%http_addr, %grpc_addr, state_dir=?state.store.path(), "stack-tun-api starting");

    let http_h = tokio::spawn(async move {
        axum::serve(http_listener, router).await.unwrap();
    });

    let grpc_h = tokio::spawn(async move {
        Server::builder()
            .add_service(
                deploy_proto::stack_tun::stack_tun_service_server::StackTunServiceServer::new(
                    grpc_svc,
                ),
            )
            .serve(grpc_addr)
            .await
            .expect("grpc server");
    });

    let _ = tokio::join!(http_h, grpc_h);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::types::*;

    #[test]
    fn connector_profile_validation() {
        let p = TunnelProfile {
            id: "p1".into(),
            name: "x".into(),
            role: TunnelRole::Connector,
            mode: TunnelMode::default(),
            link_kind: TunnelLinkKind::default(),
            source_node_id: None,
            target_node_id: None,
            route_tags: vec![],
            allowed_hosts: vec![],
            allowed_paths: vec![],
            route_priority: None,
            default_bus_decision: None,
            listen_addr: None,
            remote_grpc_endpoint: Some("http://127.0.0.1:9381".into()),
            listen_profile_id: Some("lid".into()),
            target_host: "127.0.0.1".into(),
            target_port: 80,
            max_pending_streams: 8,
            stream_offer_ttl_secs: 60,
            pull_wait_ms: 3000,
            connector_allow_pubkey_b64: Vec::new(),
            enabled: true,
            max_pending_tasks: None,
            task_queue_ttl_secs: None,
            task_claim_lease_secs: None,
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn listen_profile_ignores_target_fields() {
        let p = TunnelProfile {
            id: "l1".into(),
            name: "listen".into(),
            role: TunnelRole::Listen,
            mode: TunnelMode::default(),
            link_kind: TunnelLinkKind::default(),
            source_node_id: None,
            target_node_id: None,
            route_tags: vec![],
            allowed_hosts: vec![],
            allowed_paths: vec![],
            route_priority: None,
            default_bus_decision: None,
            listen_addr: Some("0.0.0.0:9000".into()),
            remote_grpc_endpoint: None,
            listen_profile_id: None,
            target_host: String::new(),
            target_port: 0,
            max_pending_streams: 8,
            stream_offer_ttl_secs: 120,
            pull_wait_ms: 5000,
            connector_allow_pubkey_b64: Vec::new(),
            enabled: true,
            max_pending_tasks: None,
            task_queue_ttl_secs: None,
            task_claim_lease_secs: None,
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn serde_persist_profiles_roundtrip() {
        let p = PersistRoot {
            version: 1,
            routes: vec![],
            profiles: vec![TunnelProfile {
                id: "l1".into(),
                name: "listen".into(),
                role: TunnelRole::Listen,
                mode: TunnelMode::default(),
                link_kind: TunnelLinkKind::default(),
                source_node_id: None,
                target_node_id: None,
                route_tags: vec![],
                allowed_hosts: vec![],
                allowed_paths: vec![],
                route_priority: None,
                default_bus_decision: None,
                listen_addr: Some("0.0.0.0:9000".into()),
                remote_grpc_endpoint: None,
                listen_profile_id: None,
                target_host: "127.0.0.1".into(),
                target_port: 80,
                max_pending_streams: 8,
                stream_offer_ttl_secs: 120,
                pull_wait_ms: 5000,
                connector_allow_pubkey_b64: Vec::new(),
                enabled: true,
                max_pending_tasks: None,
                task_queue_ttl_secs: None,
                task_claim_lease_secs: None,
            }],
        };
        let s = serde_json::to_string(&p).unwrap();
        let q: PersistRoot = serde_json::from_str(&s).unwrap();
        assert_eq!(q.profiles.len(), 1);
    }
}
