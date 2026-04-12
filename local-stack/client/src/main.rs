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
use deploy_client::{
    default_version, deploy_directory, fetch_server_stack_info, read_or_pack_bundle,
    upload_server_stack_artifact_with_progress, validate_version_label,
};
use deploy_proto::deploy::{PairRequest, RollbackRequest};
use deploy_proto::DeployServiceClient;
use rand_core::{OsRng, RngCore};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    /// Print `pirate` (deploy-client) version; with `--endpoint` / `--url`, also query server stack versions (GetServerStackInfo).
    #[arg(short = 'V', long = "version", global = true)]
    version: bool,

    /// Print versions of all crates linked into this binary; with `--endpoint` / `--url`, also query server stack versions (GetServerStackInfo).
    #[arg(long = "version-all", global = true)]
    version_all: bool,

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
    /// Pair with the server (install JSON), then call GetStatus and print RTT.
    ///
    /// `current_version` is the active **app** release (symlink `current` under the deploy root). Until the first `deploy`, the server may return `stack@…` (install bundle / binary lineage) or an empty string (shown as `(none)` on older servers).
    ///
    /// Lines such as `server_stack_bundle` / `deploy_server_binary` come from GetServerStackInfo (server install metadata), not from the app release.
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
    /// Register this machine's public key with the server (install JSON bundle) only; does not print GetStatus.
    ///
    /// Use `auth` for pair + status, or run `status` after pairing to see `current_version`.
    Pair {
        /// JSON from server logs: {"token":"...","url":"...","pairing":"..."} or path to a file containing it.
        #[arg(long)]
        bundle: Option<String>,
    },
    /// Create tar.gz from a directory and upload in chunks.
    Deploy {
        /// Directory to pack (e.g. ./build).
        path: PathBuf,
        /// Release version label (must match server rules: [a-zA-Z0-9._-]); not `--version` (reserved for `-V` / `--version`).
        #[arg(long = "release")]
        release: Option<String>,
        /// Chunk size in bytes for streaming.
        #[arg(long, default_value_t = 64 * 1024)]
        chunk_size: usize,
    },
    /// Query deployed app version (`current_version`) and process state from GetStatus.
    ///
    /// Empty `current_version` means no app release yet; run `deploy` (see `auth` help).
    Status,
    /// Switch to an existing release and restart the app.
    Rollback {
        /// Target version directory name under releases/.
        #[arg(value_name = "RELEASE")]
        release: String,
    },
    /// OTA: upload a server-stack bundle tarball (or pack a directory) to `UploadServerStack` on the given server.
    ///
    /// Use global `--url` / `--endpoint` for the deploy-server gRPC address (e.g. `http://192.168.0.30:50051`). Requires prior `auth` to that host when signing is enabled.
    Update {
        /// Path to `.tar.gz` / `.tgz`, or a directory to pack (same layout as `build-linux-bundle.sh`).
        path: PathBuf,
        /// Stack version label sent to the server (must match `[a-zA-Z0-9._-]`). If omitted for a `.tar.gz` / `.tgz` file, derived from the filename stem (without `.tar.gz`).
        #[arg(long = "release")]
        release: Option<String>,
        /// Chunk size in bytes for streaming.
        #[arg(long, default_value_t = 64 * 1024)]
        chunk_size: usize,
    },
}

/// `GetStatus.current_version`: empty until the first app release (`deploy` creates `releases/<ver>` and `current`).
/// When the server has no app yet, it may return `stack@…` (install metadata — not a `releases/` label).
fn display_current_version(v: &str) -> &str {
    if v.is_empty() {
        "(none)"
    } else {
        v
    }
}

fn no_deployed_app_release(current_version: &str) -> bool {
    current_version.is_empty() || current_version.starts_with("stack@")
}

/// Version label for `UploadServerStack` (proto `ServerStackChunk.version`).
fn stack_update_version_label(
    path: &Path,
    release: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(v) = release {
        validate_version_label(v)?;
        return Ok(v.to_string());
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let base = name
        .strip_suffix(".tar.gz")
        .or_else(|| name.strip_suffix(".tgz"))
        .unwrap_or("");
    if !base.is_empty() && base != name {
        validate_version_label(base)?;
        return Ok(base.to_string());
    }
    if path.is_dir() {
        return Ok(default_version());
    }
    Err("pass --release <label> when the bundle path is not a .tar.gz / .tgz file".into())
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

async fn run_auth(
    bundle_arg: Option<String>,
    project_id: String,
) -> Result<(), Box<dyn std::error::Error>> {
    run_pair(bundle_arg).await?;
    let conn = load_connection().ok_or("internal: missing connection after pair")?;
    let url = normalize_endpoint(&conn.url);
    let sk = load_or_create_identity()?;
    let mut client = DeployServiceClient::connect(url.clone()).await?;
    let mut req = Request::new(deploy_proto::deploy::StatusRequest {
        project_id: project_id.clone(),
    });
    attach_auth_metadata(&mut req, &sk, "GetStatus", &project_id, "")?;
    let t0 = Instant::now();
    let r = client.get_status(req).await?.into_inner();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    if no_deployed_app_release(&r.current_version) {
        println!(
            "rtt_ms={:.2} current_version={} state={}",
            ms,
            display_current_version(&r.current_version),
            r.state
        );
        eprintln!(
            "note: no app release deployed yet (no `current` symlink under the deploy root); run `deploy` when ready."
        );
        if r.current_version.starts_with("stack@") {
            eprintln!(
                "note: `stack@…` in current_version is server install metadata from GetStatus, not an app release label."
            );
        }
        let mut printed_stack_meta = false;
        match fetch_server_stack_info(&url, Some(&sk)).await {
            Ok(info) => {
                if !info.bundle_version.is_empty() {
                    println!("server_stack_bundle={}", info.bundle_version);
                    printed_stack_meta = true;
                }
                if let Some(ref v) = info.deploy_server_binary_version {
                    println!("deploy_server_binary={v}");
                    printed_stack_meta = true;
                }
            }
            Err(e) => eprintln!("note: could not read server stack info: {e}"),
        }
        if printed_stack_meta {
            eprintln!(
                "note: server_stack_bundle / deploy_server_binary are server install metadata; the deployed app release is set by `deploy` and appears as plain `current_version` (no `stack@` prefix)."
            );
        }
    } else {
        println!(
            "rtt_ms={:.2} current_version={} state={}",
            ms, r.current_version, r.state
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if cli.version_all {
        version_info::run_version_all(&cli).await?;
        return Ok(());
    }

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
            run_auth(bundle.clone(), cli.project.clone()).await?;
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
            release,
            chunk_size,
        } => {
            let endpoint = resolve_endpoint_normalized(&cli);
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                eprintln!("endpoint must start with http:// or https://");
                std::process::exit(2);
            }

            let version = release.clone().unwrap_or_else(default_version);
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
            println!(
                "current_version={} state={}",
                display_current_version(&r.current_version),
                r.state
            );
            if no_deployed_app_release(&r.current_version) {
                eprintln!(
                    "note: no app release deployed yet (no `current` symlink under the deploy root); run `deploy` when ready."
                );
                if r.current_version.starts_with("stack@") {
                    eprintln!(
                        "note: `stack@…` is server install metadata, not an app release under `releases/`."
                    );
                }
            }
        }
        Commands::Rollback { ref release } => {
            validate_version_label(release)?;
            let endpoint = resolve_endpoint_normalized(&cli);
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                eprintln!("endpoint must start with http:// or https://");
                std::process::exit(2);
            }
            let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
            let mut req = Request::new(RollbackRequest {
                version: release.to_string(),
                project_id: cli.project.clone(),
            });
            if use_signed_requests(&endpoint) {
                let sk = load_or_create_identity()?;
                attach_auth_metadata(&mut req, &sk, "Rollback", &cli.project, release)?;
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
        Commands::Update {
            path,
            release,
            chunk_size,
        } => {
            let endpoint = resolve_endpoint_normalized(&cli);
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                eprintln!("endpoint must start with http:// or https://");
                std::process::exit(2);
            }

            let version = stack_update_version_label(path.as_path(), release.as_deref())?;
            eprintln!(
                "server-stack update → {} (version label: {})",
                endpoint, version
            );
            eprintln!("reading {} …", path.display());
            let artifact = read_or_pack_bundle(path.as_path()).map_err(|e| {
                format!("cannot read bundle {}: {e}", path.display())
            })?;
            let total = artifact.len() as u64;
            eprintln!("artifact_bytes={total}");

            let sk = if use_signed_requests(&endpoint) {
                Some(load_or_create_identity()?)
            } else {
                None
            };

            let last_shown_pct: Arc<Mutex<i32>> = Arc::new(Mutex::new(-1));
            let last_pct = Arc::clone(&last_shown_pct);
            let on_progress = move |sent: u64, total: u64| {
                if total == 0 {
                    return;
                }
                let pct = ((sent.saturating_mul(100)) / total).min(100) as i32;
                let done = sent >= total;
                {
                    let mut last = last_pct.lock().unwrap();
                    if !done && pct <= *last {
                        return;
                    }
                    *last = pct;
                }
                let width = 36usize;
                let filled = ((sent as f64 / total as f64) * width as f64).round() as usize;
                let filled = filled.min(width);
                let bar = "█".repeat(filled) + &"░".repeat(width.saturating_sub(filled));
                let mut err = std::io::stderr().lock();
                let _ = write!(
                    err,
                    "\rupload  [{bar}] {pct:3}%  {sent} / {total} bytes"
                );
                let _ = err.flush();
            };

            let r = match upload_server_stack_artifact_with_progress(
                &endpoint,
                &artifact,
                &version,
                *chunk_size,
                sk.as_ref(),
                on_progress,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = writeln!(std::io::stderr());
                    if !use_signed_requests(&endpoint)
                        && e.code() == Code::Unauthenticated
                        && e.message().contains("missing metadata")
                    {
                        eprintln!(
                            "hint: run `pirate auth '<JSON>'` first (see install output or journalctl -u deploy-server)."
                        );
                        eprintln!(
                            "      Without pair, no x-deploy-pubkey is sent; endpoint was: {}",
                            endpoint
                        );
                    }
                    eprintln!("update: FAILED ({})", e.message());
                    std::process::exit(1);
                }
            };

            let _ = writeln!(std::io::stderr());
            println!("update: OK");
            println!(
                "status={} applied_version={}",
                r.status, r.applied_version
            );
            if let Some(ref v) = r.deploy_server_pkg_version {
                println!("deploy_server_pkg_version={v}");
            }
            if let Some(ref v) = r.control_api_pkg_version {
                println!("control_api_pkg_version={v}");
            }
        }
    }

    Ok(())
}
