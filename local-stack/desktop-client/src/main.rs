//! Pirate Client — local desktop UI on 127.0.0.1 only.

use clap::Parser;
use pirate_desktop::hosts;
use pirate_desktop::port;
use pirate_desktop::server;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser, Debug)]
#[command(
    name = "pirate-client",
    version,
    about = "Local web UI for Pirate Client (127.0.0.1 only)"
)]
struct Cli {
    /// Do not open the system browser.
    #[arg(long)]
    no_browser: bool,

    /// Directory with built static UI (index.html + assets). Also set PIRATE_DESKTOP_UI.
    #[arg(long)]
    ui_dir: Option<PathBuf>,
}

fn resolve_ui_dir(cli: &Cli) -> PathBuf {
    if let Some(ref p) = cli.ui_dir {
        return p.clone();
    }
    if let Ok(p) = std::env::var("PIRATE_DESKTOP_UI") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let ui = dir.join("ui");
            if ui.join("index.html").is_file() {
                return ui;
            }
        }
    }
    #[cfg(debug_assertions)]
    {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop-ui/dist");
        if dev.join("index.html").is_file() {
            return dev;
        }
    }
    eprintln!(
        "Pirate Client: UI not found.\n\
         Build:  cd local-stack/desktop-ui && npm install && npm run build\n\
         Or set PIRATE_DESKTOP_UI to the dist folder, or place ui/index.html next to this binary."
    );
    std::process::exit(1);
}

fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(&log_dir, "pirate-client.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(true),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .init();

    tracing::info!(log_dir = %log_dir.display(), "logging to file and stdout");
    guard
}

#[tokio::main]
async fn main() {
    let _guard = init_tracing();
    let cli = Cli::parse();
    let ui_dir = resolve_ui_dir(&cli);

    loop {
        match run_session(&cli, ui_dir.clone()).await {
            Ok(()) => {
                tracing::info!("server stopped cleanly");
                break;
            }
            Err(e) => {
                tracing::error!(error = %e, "server error; restarting in 2s");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn run_session(
    cli: &Cli,
    ui_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = port::bind_first_available().await?;
    let port_num = listener.local_addr()?.port();
    let hosts_ok = hosts::try_ensure_hosts_mapping();

    let open_url = if hosts_ok {
        format!("http://{}:{}", hosts::HOSTNAME, port_num)
    } else {
        format!("http://127.0.0.1:{}", port_num)
    };

    if !cli.no_browser {
        match open::that(&open_url) {
            Ok(()) => tracing::info!(%open_url, "opened browser"),
            Err(e) => {
                tracing::warn!(error = %e, %open_url, "could not open browser — open the URL manually")
            }
        }
    } else {
        tracing::info!(%open_url, "browser launch skipped (--no-browser)");
    }

    server::serve(listener, ui_dir, hosts_ok).await?;
    Ok(())
}
