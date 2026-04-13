//! Write sing-box JSON from the metadata DB and optionally run a reload hook (`INGRESS_RELOAD_CMD`).

use clap::Parser;
use deploy_db::DbStore;
use ingress_config::{build_singbox_config, SingboxBuildOptions};
use std::process::Stdio;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "ingress-manager")]
struct Args {
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[arg(long, default_value = "/etc/sing-box/config.json")]
    output: std::path::PathBuf,
    /// Shell command to run after a successful write (e.g. `systemctl reload sing-box`).
    #[arg(long, env = "INGRESS_RELOAD_CMD")]
    reload_cmd: Option<String>,
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
    let db = DbStore::connect(&args.database_url).await?;
    let rows = db.list_grpc_proxy_sessions_page(10_000, 0, Some(false)).await?;
    let opts = SingboxBuildOptions::default();
    let doc = build_singbox_config(&rows, &opts)?;
    let text = serde_json::to_string_pretty(&doc)?;
    if let Some(parent) = args.output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&args.output, &text)?;
    info!(path = %args.output.display(), bytes = text.len(), "wrote sing-box config");

    if let Some(cmd) = args.reload_cmd.as_ref().filter(|s| !s.trim().is_empty()) {
        let r = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await;
        match r {
            Ok(s) if s.success() => info!("reload command completed"),
            Ok(s) => warn!(code = ?s.code(), "reload command failed"),
            Err(e) => warn!(%e, "reload command spawn failed"),
        }
    }

    Ok(())
}
