//! gRPC deploy server — default bind is `::` (all interfaces); use `--bind 0.0.0.0` in Docker if needed.

mod admin_seed;
mod auth;
mod benchmark;
mod deploy_service;
mod tunnel_admission;
mod tunnel_flush;
mod tunnel_registry;
mod wire_relay;
mod metrics_http;
mod proxy_session;
mod quic;
mod session_audit;

use clap::{Parser, Subcommand};
use deploy_auth::format_install_bundle;
use deploy_control::NetCounters;
use deploy_db::DbStore;
use deploy_proto::deploy::deploy_service_server::DeployServiceServer;
use deploy_service::DeployServiceImpl;
use metrics_http::{serve_metrics_loop, ProxyTunnelMetrics};
use tunnel_admission::TunnelAdmission;
use tunnel_registry::redis_optional_from_env;
use futures_util::stream::TryStreamExt;
use session_audit::{AuditedTcpStream, SessionAuditHub};
use std::collections::HashMap;
use tokio::net::TcpListener;
use tonic::transport::server::TcpIncoming;
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
    /// Create or update a dashboard user (Argon2 hash in `dashboard_users`). Requires metadata DB URL.
    #[command(name = "dashboard-add-user")]
    DashboardAddUser(DashboardUserArgs),
    /// Run CPU/RAM/disk (and optional GPU) benchmarks; store scores 0–1000 in the metadata DB.
    #[command(name = "resource-benchmark")]
    ResourceBenchmark,
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

    /// Maximum size (bytes) for server-stack OTA tarball (`UploadServerStack`).
    #[arg(long, env = "DEPLOY_MAX_SERVER_STACK_BYTES", default_value_t = 512 * 1024 * 1024)]
    max_server_stack_bytes: u64,

    /// Allow `UploadServerStack` (requires apply helper: `pirate-apply-stack-bundle.sh` + sudo on Unix, or `pirate-apply-stack-bundle.ps1` on Windows).
    ///
    /// Do **not** wire `DEPLOY_ALLOW_SERVER_STACK_UPDATE` through clap's `env = …`: clap only accepts `true`/`false`
    /// for bool flags, while `/etc/pirate-deploy.env` commonly uses `1`/`0`. The server reads that variable in `main`
    /// after parse (see `allow_server_stack_update_effective` below).
    #[arg(long, default_value_t = false)]
    allow_server_stack_update: bool,

    /// Fallback executable name inside a release if `run.sh` is missing.
    #[arg(long, default_value = "app")]
    binary_name: String,

    /// Metadata SQLite file URL (native install), e.g. `sqlite:///var/lib/pirate/deploy/deploy.db`.
    #[arg(long, env = "DEPLOY_SQLITE_URL")]
    deploy_sqlite_url: Option<String>,

    /// Metadata PostgreSQL URL for audit/UI (Docker / optional; IPv6: `postgresql://user:pass@[::1]:5432/db`).
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

    /// Optional `host:port` for Prometheus text metrics (`GET /metrics`). Env: `DEPLOY_METRICS_BIND`.
    #[arg(long, env = "DEPLOY_METRICS_BIND")]
    metrics_bind: Option<String>,

    /// UDP bind for QUIC proxy data-plane. Env: `DEPLOY_QUIC_BIND`. Default `0.0.0.0:7844`; empty string disables.
    #[arg(long, env = "DEPLOY_QUIC_BIND")]
    quic_bind: Option<String>,

    /// PEM certificate path for QUIC TLS (optional; ephemeral self-signed if omitted).
    #[arg(long, env = "DEPLOY_QUIC_TLS_CERT")]
    quic_tls_cert: Option<PathBuf>,

    /// PEM private key path for QUIC TLS (optional).
    #[arg(long, env = "DEPLOY_QUIC_TLS_KEY")]
    quic_tls_key: Option<PathBuf>,
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

/// CLI flag `--allow-server-stack-update` **or** env `DEPLOY_ALLOW_SERVER_STACK_UPDATE` (`1`, `true`, case-insensitive).
fn allow_server_stack_update_effective(cli_flag: bool) -> bool {
    cli_flag
        || std::env::var("DEPLOY_ALLOW_SERVER_STACK_UPDATE")
            .map(|v| {
                let t = v.trim();
                t == "1"
                    || t.eq_ignore_ascii_case("true")
                    || t.eq_ignore_ascii_case("yes")
            })
            .unwrap_or(false)
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

fn grpc_host_from_public_url(url: &str) -> String {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let end = rest
        .find(|c| c == ':' || c == '/')
        .unwrap_or(rest.len());
    if end == 0 {
        return "127.0.0.1".to_string();
    }
    rest[..end].to_string()
}

fn resolve_quic_bind(args: &Args) -> Option<SocketAddr> {
    let bind_str = args
        .quic_bind
        .clone()
        .or_else(|| std::env::var("DEPLOY_QUIC_BIND").ok())
        .unwrap_or_else(|| "0.0.0.0:7844".to_string());
    let s = bind_str.trim();
    if s.is_empty() {
        return None;
    }
    s.parse().ok()
}

fn metadata_database_url(args: &Args) -> Option<String> {
    args.deploy_sqlite_url
        .clone()
        .or_else(|| args.database_url.clone())
        .filter(|s| !s.trim().is_empty())
}

async fn resource_benchmark_cmd(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    ensure_not_root()?;
    let Some(ref url) = metadata_database_url(args) else {
        return Err(
            "deploy-server resource-benchmark: set DEPLOY_SQLITE_URL or DATABASE_URL".into(),
        );
    };
    let store = DbStore::connect(url).await?;
    store.migrate().await?;
    benchmark::run_resource_benchmark(&store).await?;
    println!("ok: resource benchmark row inserted");
    Ok(())
}

async fn dashboard_add_user_cmd(
    args: &Args,
    uargs: &DashboardUserArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_not_root()?;
    let Some(ref url) = metadata_database_url(args) else {
        return Err(
            "deploy-server dashboard-add-user: set DEPLOY_SQLITE_URL or DATABASE_URL".into(),
        );
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
    if matches!(top.command, Some(Commands::ResourceBenchmark)) {
        return resource_benchmark_cmd(&top.run).await;
    }

    let args = top.run;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    ensure_not_root()?;
    let allow_server_stack_update =
        allow_server_stack_update_effective(args.allow_server_stack_update);

    info!(
        root = %args.root.display(),
        port = args.port,
        max_upload_bytes = args.max_upload_bytes,
        max_server_stack_bytes = args.max_server_stack_bytes,
        allow_server_stack_update = allow_server_stack_update,
        binary = %args.binary_name,
        database = %if metadata_database_url(&args).is_some() { "configured" } else { "disabled" },
        "starting deploy-server"
    );

    let db = if let Some(ref url) = metadata_database_url(&args) {
        let store = DbStore::connect(url).await?;
        store.migrate().await?;
        if store.is_postgres() {
            info!("PostgreSQL metadata migrations applied");
        } else {
            info!("SQLite metadata migrations applied");
        }
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
    let host_net = Arc::new(std::sync::Mutex::new(None::<NetCounters>));
    let log_tail_path = std::env::var("DEPLOY_HOST_STATS_LOG_TAIL")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let session_hub = SessionAuditHub::new(db.clone());
    let proxy_metrics = Arc::new(ProxyTunnelMetrics::default());
    let tunnel_admission =
        TunnelAdmission::new(tunnel_admission::max_concurrent_from_env(), proxy_metrics.clone());
    let tunnel_redis = redis_optional_from_env();

    let quic_dataplane: Option<crate::quic::QuicDataplaneState> =
        if crate::quic::dataplane_enabled() {
            if let Some(addr) = resolve_quic_bind(&args) {
                match crate::quic::start_quic_listener(
                    addr,
                    args.quic_tls_cert.as_deref(),
                    args.quic_tls_key.as_deref(),
                ) {
                    Ok((endpoint, store)) => {
                        let host = grpc_host_from_public_url(&public_url);
                        let port = std::env::var("DEPLOY_QUIC_PUBLIC_PORT")
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(7844u16);
                        let st = crate::quic::QuicDataplaneState {
                            ticket_store: store.clone(),
                            public_host: host,
                            public_port: port,
                        };
                        tokio::spawn(crate::quic::run_quic_accept_loop(endpoint, store));
                        info!(%addr, quic_public = %format!("{}:{}", st.public_host, st.public_port), "QUIC data-plane listening");
                        Some(st)
                    }
                    Err(e) => {
                        info!(error = %e, "QUIC data-plane listener not started");
                        None
                    }
                }
            } else {
                info!("QUIC data-plane disabled (empty DEPLOY_QUIC_BIND)");
                None
            }
        } else {
            None
        };

    if let Some(s) = args.metrics_bind.as_ref().map(|x| x.trim()).filter(|s| !s.is_empty()) {
        if let Ok(sock) = s.parse::<SocketAddr>() {
            let m = proxy_metrics.clone();
            tokio::spawn(serve_metrics_loop(sock, m));
        } else {
            info!(bind = %s, "DEPLOY_METRICS_BIND: invalid address, metrics disabled");
        }
    }
    let svc = DeployServiceServer::new(DeployServiceImpl::new(
        args.root,
        args.max_upload_bytes,
        args.max_server_stack_bytes,
        allow_server_stack_update,
        args.binary_name,
        public_url,
        states,
        db,
        server_auth,
        host_net,
        log_tail_path,
        session_hub.clone(),
        proxy_metrics,
        tunnel_admission,
        tunnel_redis,
        quic_dataplane,
    ));

    let listener = TcpListener::bind(addr).await?;
    let tcp_incoming =
        TcpIncoming::from_listener(listener, true, None).map_err(|e| format!("listen: {e}"))?;
    let incoming = tcp_incoming.map_ok({
        let hub = session_hub.clone();
        move |tcp| AuditedTcpStream::new(tcp, hub.clone())
    });

    tonic::transport::Server::builder()
        .add_service(svc)
        .serve_with_incoming(incoming)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;

    Ok(())
}
