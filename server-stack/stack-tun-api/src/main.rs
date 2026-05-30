//! stack-tun-api — queued TCP relays between hosts (listen + connector roles).

use stack_tun_api::{run, StackTunArgs};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = StackTunArgs::parse();
    run(args).await.map_err(|e| e.into())
}
