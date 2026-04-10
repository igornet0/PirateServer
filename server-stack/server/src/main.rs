//! gRPC deploy server — default bind is `::` (all interfaces); use `--bind 0.0.0.0` in Docker if needed.

mod deploy_service;

use clap::Parser;
use deploy_core::AppState;
use deploy_db::DbStore;
use deploy_proto::deploy::deploy_service_server::DeployServiceServer;
use deploy_service::DeployServiceImpl;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "deploy-server", about = "Artifact deploy server (gRPC, IPv6)")]
struct Args {
    /// Root directory for deploy layout (`releases/`, `current` symlink).
    #[arg(long, default_value = "/deploy")]
    root: PathBuf,

    /// Bind address (`::` all IPv6 interfaces, dual-stack where supported; `0.0.0.0` for all IPv4).
    #[arg(long, default_value = "::")]
    bind: IpAddr,

    /// gRPC listen port.
    #[arg(short, long, default_value_t = 50051)]
    port: u16,

    /// Maximum artifact size (bytes) for a single upload.
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    max_upload_bytes: u64,

    /// Fallback executable name inside a release if `run.sh` is missing.
    #[arg(long, default_value = "app")]
    binary_name: String,

    /// Optional PostgreSQL URL for audit/UI (IPv6 host: `postgresql://user:pass@[::1]:5432/db`).
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,
}

#[cfg(unix)]
fn ensure_not_root() -> Result<(), Box<dyn std::error::Error>> {
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        return Err("refusing to run as root; start as a non-privileged user".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_not_root() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    ensure_not_root()?;
    info!(
        root = %args.root.display(),
        port = args.port,
        max_upload_bytes = args.max_upload_bytes,
        binary = %args.binary_name,
        database = %if args.database_url.is_some() { "configured" } else { "disabled" },
        "starting deploy-server"
    );

    let db = if let Some(ref url) = args.database_url {
        let store = DbStore::connect(url).await?;
        store.migrate().await?;
        info!("PostgreSQL migrations applied");
        Some(Arc::new(store))
    } else {
        None
    };

    let addr = SocketAddr::new(args.bind, args.port);
    info!(listen = %addr, "listening");

    let state = Arc::new(tokio::sync::Mutex::new(AppState::default()));
    let svc = DeployServiceServer::new(DeployServiceImpl::new(
        args.root,
        args.max_upload_bytes,
        args.binary_name,
        state,
        db,
    ));

    tonic::transport::Server::builder()
        .add_service(svc)
        .serve(addr)
        .await?;

    Ok(())
}
