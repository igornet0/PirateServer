//! Out-of-band lifecycle agent (server-stack OTA + reboot).

use clap::Parser;
use pirate_host_agent::{run_server, AgentConfig, AgentState};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "pirate-host-agent")]
struct Args {
    /// Listen address (e.g. 127.0.0.1:9443 or 0.0.0.0:9443)
    #[arg(long, env = "PIRATE_HOST_AGENT_BIND", default_value = "127.0.0.1:9443")]
    bind: String,

    /// Bearer token for /v1/* (required)
    #[arg(long, env = "PIRATE_HOST_AGENT_TOKEN")]
    token: String,

    /// Deploy root (staging under .host-agent-staging)
    #[arg(long, env = "PIRATE_HOST_AGENT_DEPLOY_ROOT", default_value = "/var/lib/pirate/deploy")]
    deploy_root: PathBuf,

    #[arg(long, env = "PIRATE_HOST_AGENT_MAX_STACK_BYTES", default_value_t = 536870912)]
    max_stack_bytes: u64,

    #[arg(long, env = "PIRATE_HOST_AGENT_TLS_CERT")]
    tls_cert: Option<PathBuf>,

    #[arg(long, env = "PIRATE_HOST_AGENT_TLS_KEY")]
    tls_key: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = Args::parse();
    if args.token.trim().is_empty() {
        eprintln!("PIRATE_HOST_AGENT_TOKEN is required");
        std::process::exit(1);
    }

    let tls = match (&args.tls_cert, &args.tls_key) {
        (Some(c), Some(k)) => Some((c.clone(), k.clone())),
        (None, None) => None,
        _ => {
            eprintln!("Both PIRATE_HOST_AGENT_TLS_CERT and PIRATE_HOST_AGENT_TLS_KEY must be set, or neither.");
            std::process::exit(1);
        }
    };

    let addr: SocketAddr = args.bind.parse()?;

    let cfg = AgentConfig {
        token: args.token.into_bytes(),
        deploy_root: args.deploy_root,
        max_stack_bytes: args.max_stack_bytes,
    };

    let state = AgentState {
        cfg,
        start: std::time::Instant::now(),
    };

    run_server(addr, state, tls).await
}
