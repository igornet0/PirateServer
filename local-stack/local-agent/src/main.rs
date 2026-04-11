//! Workstation-side agent: outbound tunnel to server `tunnel-gateway` (see `src/tunnel.rs`).

mod tunnel;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "local-agent",
    version,
    about = "PC ↔ server tunnel (HTTP to control-api via tunnel-gateway)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print tunnel usage (no network).
    Info,
    /// Outbound TCP to tunnel-gateway; serves local HTTP that is proxied to server control-api.
    Tunnel {
        /// tunnel-gateway address, e.g. `192.0.2.1:8445`
        #[arg(long)]
        server: String,
        /// Local bind for HTTP clients (e.g. curl).
        #[arg(long, default_value = "127.0.0.1:9999")]
        local: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().command {
        Some(Commands::Tunnel { server, local }) => {
            tunnel::run_tunnel(&server, &local).await?;
        }
        Some(Commands::Info) | None => tunnel::print_info(),
    }
    Ok(())
}
