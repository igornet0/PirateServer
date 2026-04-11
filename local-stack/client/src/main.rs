//! CLI: pack `build/` as tar.gz, stream over gRPC (IPv6 endpoint); optional Ed25519 pairing.

mod board;
mod config;
mod version_info;

use clap::{CommandFactory, Parser, Subcommand};
use config::{
    load_connection, load_or_create_identity, normalize_endpoint, save_connection,
    use_signed_requests, StoredConnection,
};
use deploy_auth::{
    attach_auth_metadata, pair_request_canonical, pubkey_b64_url, verify_pair_response,
    ConnectionBundle, now_unix_ms,
};
use deploy_client::{default_version, deploy_directory, validate_version_label};
use deploy_proto::deploy::{PairRequest, RollbackRequest};
use deploy_proto::DeployServiceClient;
use rand_core::{OsRng, RngCore};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tonic::Code;
use tonic::Request;

/// Default gRPC HTTP/2 endpoint (IPv6 loopback).
const DEFAULT_ENDPOINT: &str = "http://[::1]:50051";

#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    about = "Deploy artifact to deploy-server over gRPC; pair with `auth`, optional HTTP CONNECT proxy with `board`",
    subcommand_required = false,
    disable_version_flag = true
)]
struct Cli {
    /// Print client version; with `--endpoint` / `--url`, also query server stack versions (GetServerStackInfo).
    #[arg(short = 'V', long = "version", global = true)]
    version: bool,

    /// Server endpoint, e.g. http://[::1]:50051 (overrides saved connection).
    #[arg(long = "endpoint", visible_alias = "url", global = true)]
    endpoint: Option<String>,

    /// Deploy target project id (`default` is the legacy single-root layout).
    #[arg(long, global = true, default_value = "default")]
    project: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Register this machine with the server (install JSON), verify with GetStatus, print RTT.
    Auth {
        /// JSON `{"token":"...","url":"...","pairing":"..."}`, path to a file, or omit for stdin.
        bundle: Option<String>,
    },
    /// Local HTTP CONNECT proxy → server outbound TCP (requires prior `auth`).
    Board {
        /// gRPC endpoint (must match saved paired connection).
        #[arg(long)]
        url: String,
        /// Listen address for the HTTP proxy (CONNECT).
        #[arg(long, default_value = "127.0.0.1:3128")]
        listen: String,
    },
    /// Register this machine's public key with the server using the install JSON bundle.
    Pair {
        /// JSON from server logs: {"token":"...","url":"...","pairing":"..."} or path to a file containing it.
        #[arg(long)]
        bundle: Option<String>,
    },
    /// Create tar.gz from a directory and upload in chunks.
    Deploy {
        /// Directory to pack (e.g. ./build).
        path: PathBuf,
        /// Release version label (must match server rules: [a-zA-Z0-9._-]).
        #[arg(long)]
        version: Option<String>,
        /// Chunk size in bytes for streaming.
        #[arg(long, default_value_t = 64 * 1024)]
        chunk_size: usize,
    },
    /// Query current version and process state.
    Status,
    /// Switch to an existing release and restart the app.
    Rollback {
        /// Target version directory name under releases/.
        version: String,
    },
}

fn resolve_endpoint(cli: &Cli) -> String {
    cli.endpoint
        .clone()
        .or_else(|| load_connection().map(|c| c.url))
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
}

fn resolve_endpoint_normalized(cli: &Cli) -> String {
    normalize_endpoint(&resolve_endpoint(cli))
}

fn read_bundle_text(bundle: &Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(s) = bundle {
        let t = s.trim();
        if t.starts_with('{') {
            return Ok(t.to_string());
        }
        let path = Path::new(t);
        if path.exists() {
            return Ok(std::fs::read_to_string(path)?);
        }
        return Ok(s.clone());
    }
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    if buf.trim().is_empty() {
        return Err("paste the install bundle JSON or pass the JSON as an argument".into());
    }
    Ok(buf)
}

async fn run_pair(bundle_arg: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let raw = read_bundle_text(&bundle_arg)?;
    let b = ConnectionBundle::parse(&raw).map_err(|e| e.to_string())?;
    if !b.url.starts_with("http://") && !b.url.starts_with("https://") {
        return Err("bundle url must start with http:// or https://".into());
    }
    let pairing = b
        .pairing_code
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or("bundle must include pairing code from server")?;

    let sk = load_or_create_identity()?;
    let client_pub = pubkey_b64_url(&sk);
    let ts_ms = now_unix_ms();
    let nonce = format!("{:016x}", OsRng.next_u64());
    let msg = pair_request_canonical(&client_pub, &b.server_pubkey_b64, ts_ms, &nonce, &pairing);
    let client_sig = deploy_auth::sign_bytes(&sk, &msg);

    let url = normalize_endpoint(&b.url);
    let mut client = DeployServiceClient::connect(url.clone()).await?;
    let resp = client
        .pair(Request::new(PairRequest {
            client_public_key_b64: client_pub.clone(),
            timestamp_ms: ts_ms,
            nonce: nonce.clone(),
            pairing_code: pairing,
            client_signature_b64: client_sig,
        }))
        .await?
        .into_inner();

    verify_pair_response(
        &b.server_pubkey_b64,
        &client_pub,
        ts_ms,
        &nonce,
        &resp.server_signature_b64,
    )
    .map_err(|e| format!("server identity check failed: {e}"))?;

    save_connection(&StoredConnection {
        url: url.clone(),
        server_pubkey_b64: b.server_pubkey_b64,
        paired: true,
    })?;
    eprintln!("paired with server; saved connection to config dir");
    println!("status={}", resp.status);
    Ok(())
}

async fn run_auth(bundle_arg: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    run_pair(bundle_arg).await?;
    let conn = load_connection().ok_or("internal: missing connection after pair")?;
    let url = normalize_endpoint(&conn.url);
    let sk = load_or_create_identity()?;
    let mut client = DeployServiceClient::connect(url.clone()).await?;
    let mut req = Request::new(deploy_proto::deploy::StatusRequest {
        project_id: "default".to_string(),
    });
    attach_auth_metadata(&mut req, &sk, "GetStatus", "default", "")?;
    let t0 = Instant::now();
    let r = client.get_status(req).await?.into_inner();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "rtt_ms={:.2} current_version={} state={}",
        ms, r.current_version, r.state
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if cli.version {
        version_info::run_version(&cli).await?;
        return Ok(());
    }

    let Some(command) = cli.command.as_ref() else {
        Cli::command().print_help()?;
        return Ok(());
    };

    match command {
        Commands::Auth { bundle } => {
            run_auth(bundle.clone()).await?;
            return Ok(());
        }
        Commands::Board { url, listen } => {
            let url = normalize_endpoint(&url);
            if !url.starts_with("http://") && !url.starts_with("https://") {
                eprintln!("--url must start with http:// or https://");
                std::process::exit(2);
            }
            if !use_signed_requests(&url) {
                eprintln!(
                    "no paired connection for this URL. Run: pirate auth '<install-json>' first"
                );
                std::process::exit(2);
            }
            let sk = load_or_create_identity()?;
            board::run_board(listen, url.as_str(), &sk).await?;
            return Ok(());
        }
        Commands::Pair { bundle } => {
            run_pair(bundle.clone()).await?;
            return Ok(());
        }
        Commands::Deploy {
            path,
            version,
            chunk_size,
        } => {
            let endpoint = resolve_endpoint_normalized(&cli);
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                eprintln!("endpoint must start with http:// or https://");
                std::process::exit(2);
            }

            let version = version.clone().unwrap_or_else(default_version);
            validate_version_label(&version)?;

            eprintln!("packing {} …", path.display());
            let sk = if use_signed_requests(&endpoint) {
                Some(load_or_create_identity()?)
            } else {
                None
            };
            let resp = deploy_directory(
                &endpoint,
                path.as_path(),
                &version,
                &cli.project,
                *chunk_size,
                sk.as_ref(),
            )
            .await?;
            println!(
                "status={} deployed_version={} bytes={} chunks={}",
                resp.response.status,
                resp.response.deployed_version,
                resp.artifact_bytes,
                resp.chunk_count
            );
        }
        Commands::Status => {
            let endpoint = resolve_endpoint_normalized(&cli);
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                eprintln!("endpoint must start with http:// or https://");
                std::process::exit(2);
            }
            let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
            let mut req = Request::new(deploy_proto::deploy::StatusRequest {
                project_id: cli.project.clone(),
            });
            if use_signed_requests(&endpoint) {
                let sk = load_or_create_identity()?;
                attach_auth_metadata(&mut req, &sk, "GetStatus", &cli.project, "")?;
            }
            let r = match client.get_status(req).await {
                Ok(r) => r.into_inner(),
                Err(e) => {
                    if !use_signed_requests(&endpoint)
                        && e.code() == Code::Unauthenticated
                        && e.message().contains("missing metadata")
                    {
                        eprintln!(
                            "hint: run `pirate auth '<JSON>'` first (see install output or journalctl -u deploy-server)."
                        );
                        eprintln!("      Without pair, no x-deploy-pubkey is sent; endpoint was: {}", endpoint);
                    }
                    return Err(e.into());
                }
            };
            println!("current_version={} state={}", r.current_version, r.state);
        }
        Commands::Rollback { ref version } => {
            validate_version_label(version)?;
            let endpoint = resolve_endpoint_normalized(&cli);
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                eprintln!("endpoint must start with http:// or https://");
                std::process::exit(2);
            }
            let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
            let mut req = Request::new(RollbackRequest {
                version: version.to_string(),
                project_id: cli.project.clone(),
            });
            if use_signed_requests(&endpoint) {
                let sk = load_or_create_identity()?;
                attach_auth_metadata(&mut req, &sk, "Rollback", &cli.project, version)?;
            }
            let r = match client.rollback(req).await {
                Ok(r) => r.into_inner(),
                Err(e) => {
                    if !use_signed_requests(&endpoint)
                        && e.code() == Code::Unauthenticated
                        && e.message().contains("missing metadata")
                    {
                        eprintln!(
                            "hint: run `pirate auth '<JSON>'` first (see install output or journalctl -u deploy-server)."
                        );
                        eprintln!("      Without pair, no x-deploy-pubkey is sent; endpoint was: {}", endpoint);
                    }
                    return Err(e.into());
                }
            };
            println!("status={} active_version={}", r.status, r.active_version);
        }
    }

    Ok(())
}
