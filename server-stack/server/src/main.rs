//! gRPC deploy server — default bind is `::` (all interfaces); use `--bind 0.0.0.0` in Docker if needed.

mod admin_seed;
mod auth;
mod deploy_service;

use clap::{Parser, Subcommand};
use deploy_auth::format_install_bundle;
use deploy_db::DbStore;
use deploy_proto::deploy::deploy_service_server::DeployServiceServer;
use deploy_service::DeployServiceImpl;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "deploy-server",
    about = "Artifact deploy server (gRPC, IPv6)",
    subcommand_required = false
)]
struct Top {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    run: Args,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print one line of JSON for `client pair`: {"token","url","pairing"} (stdout; does not start the server).
    #[command(name = "print-install-bundle")]
    PrintInstallBundle,
    /// Create or update a dashboard user (Argon2 hash in `dashboard_users`). Requires DATABASE_URL.
    #[command(name = "dashboard-add-user")]
    DashboardAddUser(DashboardUserArgs),
}

#[derive(Parser, Debug)]
struct DashboardUserArgs {
    /// Login name (letters, digits, `.`, `_`, `-`).
    #[arg(long)]
    username: String,
    /// Plain password. If omitted, uses env `DEPLOY_DASHBOARD_PASSWORD`.
    #[arg(long)]
    password: Option<String>,
}

#[derive(Parser, Debug)]
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

    /// Directory for `server_ed25519.json`, `authorized_peers.json`, `pairing.code`. Default: `<root>/.keys`.
    #[arg(long, env = "DEPLOY_KEYS_DIR")]
    keys_dir: Option<PathBuf>,

    /// If set, gRPC does not require pairing or per-request signatures (dev/test only).
    /// Also accepts env `DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1` or `true`.
    #[arg(long, default_value_t = false)]
    allow_unauthenticated: bool,

    /// URL shown in the install bundle (reachable gRPC endpoint for clients). Default from `DEPLOY_GRPC_PUBLIC_URL` or loopback.
    #[arg(long, env = "DEPLOY_GRPC_PUBLIC_URL")]
    public_url: Option<String>,
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

fn print_install_bundle(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    ensure_not_root()?;
    let keys_dir = args
        .keys_dir
        .clone()
        .unwrap_or_else(|| args.root.join(".keys"));
    let allow_insecure = args.allow_unauthenticated
        || std::env::var("DEPLOY_GRPC_ALLOW_UNAUTHENTICATED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    let server_auth = auth::ServerAuth::init(&keys_dir, allow_insecure)?;
    let Some(sa) = server_auth else {
        eprintln!(
            "deploy-server: gRPC authentication disabled (DEPLOY_GRPC_ALLOW_UNAUTHENTICATED); no install bundle"
        );
        std::process::exit(1);
    };
    let public_url = args
        .public_url
        .clone()
        .or_else(|| {
            std::env::var("DEPLOY_GRPC_PUBLIC_URL")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", args.port));
    let code = sa.pairing_code.read().clone();
    let bundle = format_install_bundle(&sa.server_pubkey_b64, &public_url, &code);
    println!("{}", bundle);
    Ok(())
}

fn validate_dashboard_username(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err("username must be 1–64 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err("username: only ASCII letters, digits, . _ -".into());
    }
    Ok(())
}

async fn dashboard_add_user_cmd(
    args: &Args,
    uargs: &DashboardUserArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_not_root()?;
    let Some(ref url) = args.database_url else {
        return Err("deploy-server dashboard-add-user: DATABASE_URL is required".into());
    };
    let username = uargs.username.trim();
    validate_dashboard_username(username)?;
    let password = uargs
        .password
        .clone()
        .or_else(|| std::env::var("DEPLOY_DASHBOARD_PASSWORD").ok())
        .filter(|s| !s.is_empty())
        .ok_or(
            "set --password … or environment variable DEPLOY_DASHBOARD_PASSWORD",
        )?;
    let store = DbStore::connect(url).await?;
    store.migrate().await?;
    let hash = admin_seed::hash_dashboard_password(&password)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    store.upsert_dashboard_user(username, &hash).await?;
    println!("ok: dashboard user {:?}", username);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let top = Top::parse();
    if matches!(top.command, Some(Commands::PrintInstallBundle)) {
        return print_install_bundle(&top.run);
    }
    if let Some(Commands::DashboardAddUser(ref uargs)) = top.command {
        return dashboard_add_user_cmd(&top.run, uargs).await;
    }

    let args = top.run;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

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
        if let Err(e) = admin_seed::seed_dashboard_admin(&store).await {
            return Err(format!("dashboard admin seed: {e}").into());
        }
        Some(Arc::new(store))
    } else {
        None
    };

    let addr = SocketAddr::new(args.bind, args.port);
    info!(listen = %addr, "listening");

    let keys_dir = args
        .keys_dir
        .clone()
        .unwrap_or_else(|| args.root.join(".keys"));
    let allow_insecure = args.allow_unauthenticated
        || std::env::var("DEPLOY_GRPC_ALLOW_UNAUTHENTICATED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    let server_auth = auth::ServerAuth::init(&keys_dir, allow_insecure)?;

    let public_url = args.public_url.clone().or_else(|| {
        std::env::var("DEPLOY_GRPC_PUBLIC_URL")
            .ok()
            .filter(|s| !s.is_empty())
    }).unwrap_or_else(|| {
        // Sensible default for local / Docker port publishing
        format!("http://127.0.0.1:{}", args.port)
    });

    if let Some(ref sa) = server_auth {
        let code = sa.pairing_code.read().clone();
        let bundle = format_install_bundle(&sa.server_pubkey_b64, &public_url, &code);
        info!(%bundle, "install bundle (give clients token+url+pairing)");
    } else {
        info!("gRPC authentication disabled (DEPLOY_GRPC_ALLOW_UNAUTHENTICATED)");
    }

    let states = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let svc = DeployServiceServer::new(DeployServiceImpl::new(
        args.root,
        args.max_upload_bytes,
        args.binary_name,
        states,
        db,
        server_auth,
    ));

    tonic::transport::Server::builder()
        .add_service(svc)
        .serve(addr)
        .await?;

    Ok(())
}
