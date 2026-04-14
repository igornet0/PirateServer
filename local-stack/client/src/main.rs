//! CLI: pack `build/` as tar.gz, stream over gRPC (IPv6 endpoint); optional Ed25519 pairing.

mod board_probe;
mod display_stream;
mod stack_update_prompt;
mod version_info;
mod gui_probe;

use clap::{CommandFactory, Parser, Subcommand};
use deploy_client::{
    board,
    bootstrap_apply,
    config::{
        identity_path, load_connection, load_or_create_identity, normalize_endpoint, save_connection,
        settings_path, use_signed_requests, StoredConnection,
    },
    connection_manager::ConnectionManager,
    settings,
};
use deploy_auth::{
    attach_auth_metadata, endpoints_equivalent_for_signing, load_identity, pair_request_canonical,
    pubkey_b64_url, verify_pair_response, ConnectionBundle, now_unix_ms,
};
use deploy_client::{
    default_version, deploy_directory, fetch_server_stack_info, inspect_bundle_path,
    read_or_pack_bundle, upload_server_stack_artifact_with_progress, validate_version_label,
};
use deploy_core::display_stream::DisplayStreamConfig;
use deploy_proto::deploy::{
    ListSessionsRequest, PairRequest, QuerySessionLogsRequest, ReportResourceUsageRequest,
    RollbackRequest, UpdateConnectionProfileRequest,
};
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
    /// Print this client's URL-safe Ed25519 public key (for proxy session `recipient_client_pubkey_b64`).
    /// Fails if `identity.json` is missing (run `auth` first, or pass `--identity`).
    ShowPubkey {
        /// Path to `identity.json` (default: config dir / `identity.json`).
        #[arg(long)]
        identity: Option<PathBuf>,
    },
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
    ///
    /// Pass the deploy-server gRPC URL with global `--url` / `--endpoint` (or rely on `connection.json` from `pirate auth`).
    Board {
        /// Listen address for the HTTP proxy (CONNECT).
        #[arg(long, default_value = "127.0.0.1:3128")]
        listen: String,
        /// Run gRPC reachability, bandwidth probe, and session summary (does not start the proxy).
        #[arg(long)]
        test_connect: bool,
        /// With `--test-connect`, print one JSON line (GetStatus + ConnectionProbe only) to stdout for scripts.
        #[arg(long)]
        probe_json: bool,
        /// Upload size for `ConnectionProbe` (max 4 MiB).
        #[arg(long, default_value_t = 1024 * 1024)]
        probe_upload_bytes: u64,
        /// Requested download payload size in the probe result (max 4 MiB).
        #[arg(long, default_value_t = 256 * 1024)]
        probe_download_bytes: u32,
        /// Board id from `settings.json` (omit: use `default_board` and host routing rules).
        #[arg(long)]
        board: Option<String>,
        /// Session token for managed `ProxyTunnel` (overrides per-board `session_token` in settings).
        #[arg(long)]
        session_token: Option<String>,
        /// Path to `settings.json` (default: pirate-client config dir).
        #[arg(long)]
        settings: Option<PathBuf>,
    },
    /// Test local HTTP CONNECT proxy: smoke GET, speed through proxy, and empirical max parallel requests (`pirate board` must be listening).
    ///
    /// Does not use gRPC `ConnectionProbe` (that measures gRPC bandwidth, not HTTP CONNECT to an upstream URL).
    #[command(name = "test-proxy")]
    TestProxy {
        /// Only measure download speed through the proxy.
        #[arg(long)]
        speed: bool,
        /// Only estimate max parallel successful HTTP requests through the proxy (synonym in docs: max_connect).
        #[arg(long = "max-connect")]
        max_connect: bool,
        /// HTTP proxy URL (local `pirate board` listener).
        #[arg(long, default_value = "http://127.0.0.1:3128")]
        proxy: String,
        /// Target URL via the proxy (default: env `PIRATE_PROXY_TEST_UPSTREAM` or `http://127.0.0.1:9000/size?bytes=262144`).
        #[arg(long)]
        upstream_url: Option<String>,
        /// Hint for `?bytes=` when the upstream URL has no `bytes=` query (speed / max-connect trials).
        #[arg(long, default_value_t = 262144)]
        bytes: u64,
        /// Per-request timeout (seconds).
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Upper bound for parallel-connection search (`max_connect` estimate).
        #[arg(long, default_value_t = 128)]
        cap: u32,
        /// Minimum success fraction for a parallel trial (0.0–1.0).
        #[arg(long, default_value_t = 0.95)]
        min_success_rate: f64,
        /// Print one JSON line (smoke, speed, max_connect fields).
        #[arg(long)]
        json: bool,
    },
    /// Register this machine's public key with the server (install JSON bundle) only; does not print GetStatus.
    ///
    /// Use `auth` for pair + status, or run `status` after pairing to see `current_version`.
    Pair {
        /// JSON from server logs: {"token":"...","url":"...","pairing":"..."} or path to a file containing it.
        #[arg(long)]
        bundle: Option<String>,
    },
    /// Merge `pirate-bootstrap` or `pirate-proxy-session` JSON into `settings.json` (per-board gRPC URL + session token).
    ///
    /// Run `pirate auth` first so `connection.json` matches this server. Fetch JSON with `--url` (public pirate-bootstrap) or read `--file`. Session auth token is only in create response or full Inbounds export — pass `--session-token` if the JSON has `session_token: null`.
    #[command(name = "bootstrap-apply")]
    BootstrapApply {
        /// Path to JSON file.
        #[arg(long)]
        file: Option<PathBuf>,
        /// HTTPS/HTTP URL to fetch bootstrap JSON (`/api/v1/public/pirate-bootstrap/...`).
        #[arg(long)]
        url: Option<String>,
        /// Session token from the create-session dialog (if not present in JSON).
        #[arg(long)]
        session_token: Option<String>,
        /// Path to `settings.json` (default: pirate-client config dir).
        #[arg(long)]
        settings: Option<PathBuf>,
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
    /// List registered gRPC client keys and last activity from the server metadata DB.
    ///
    /// Requires `DEPLOY_SQLITE_URL` / `DATABASE_URL` on deploy-server. When the server uses signed gRPC (normal install), you must run `pirate auth` first so `connection.json` matches this `--endpoint`. Use `--last-log` / `--export-log -o file.csv` for audit rows.
    Sessions {
        /// Recent session audit events (TCP open/close, pair, …), newest first.
        #[arg(long)]
        last_log: bool,
        /// Export all audit rows to CSV (paginates until exhausted). Requires `-o` / `--output`.
        #[arg(long)]
        export_log: bool,
        /// Output path for `--export-log` (CSV).
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        /// Page size for `--last-log` / `--export-log` (server max 500; default 50).
        #[arg(long, default_value_t = 50)]
        limit: i32,
    },
    /// Set this client's connection role on the server: `proxy` or `resource` (signed gRPC).
    Profile {
        /// `proxy` or `resource`
        #[arg(value_name = "KIND")]
        kind: String,
    },
    /// Report local CPU/RAM (and optional GPU) usage to the server for RESOURCE clients.
    ResourceReport,
    /// Desktop display stream: list monitors or run producer (POST JPEG to consumer ingest URL).
    DisplayStream {
        #[command(subcommand)]
        cmd: DisplayStreamCmd,
    },
    /// Detect local GUI / desktop session (JSON). Run on the deploy host; use `--remote` with `auth` to read install snapshot from the server.
    #[command(name = "gui-check")]
    GuiCheck {
        /// Include GetServerStackInfo fields `host_gui_*` from the server (requires prior `pirate auth`).
        #[arg(long)]
        remote: bool,
        /// `json` (default) or `keyval` for scripts.
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Display stream helpers: build producer config JSON for `display-stream run` (screen translation to ingest URL).
    Gui {
        #[command(subcommand)]
        cmd: GuiCmd,
    },
    /// Uninstall native server stack (Linux) or remove local CLI pairing state.
    Uninstall {
        #[command(subcommand)]
        target: UninstallTarget,
    },
    /// Create `pirate.toml` with auto-detected runtime (Node, Python, Go, …).
    #[command(name = "init-project")]
    InitProject {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Scan project markers and refresh `pirate.toml` (port/runtime hints).
    #[command(name = "scan-project")]
    ScanProject {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run `[build].cmd` from `pirate.toml` in the project directory.
    #[command(name = "project-build")]
    ProjectBuild {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Run `[test].cmd` from `pirate.toml`.
    #[command(name = "project-test")]
    ProjectTest {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Generate `Dockerfile` if missing, build image, run container, HTTP health check.
    #[command(name = "test-local")]
    TestLocal {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "pirate-local-test")]
        image: String,
    },
    /// Write `run.sh`, `docker-compose.pirate.yml`, nginx snippet from `pirate.toml`.
    #[command(name = "apply-gen")]
    ApplyGen {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Run build/test/deploy using `[project].name` from the local registry (`pirate projects add`).
    #[command(name = "project")]
    Project {
        /// Value of `[project].name` in `pirate.toml` (must match a registered project path).
        #[arg(value_name = "NAME")]
        name: String,
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Manage project name → path registry (under config dir: `pirate-projects.json`).
    Projects {
        #[command(subcommand)]
        sub: ProjectsCmd,
    },
}

/// Subcommands for `pirate project <NAME> …`
#[derive(Subcommand, Debug)]
enum ProjectCmd {
    /// Run `[build].cmd` from the project's `pirate.toml`.
    Build,
    /// Run `[test].cmd`.
    Test,
    /// Pack directory and upload (same as `deploy` but resolved by project name).
    Deploy {
        /// Optional note for this deploy (saved under `.pirate/last-deploy-message.txt`).
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        /// Release version label (same rules as `deploy --release`).
        #[arg(long = "release")]
        release: Option<String>,
        #[arg(long, default_value_t = 64 * 1024)]
        chunk_size: usize,
    },
}

#[derive(Subcommand, Debug)]
enum ProjectsCmd {
    /// Register a directory: reads `pirate.toml` and stores `[project].name` → path.
    Add {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Print registered names and paths as JSON.
    List,
    /// Remove a name from the registry (files on disk are untouched).
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum DisplayStreamCmd {
    /// List monitors (index, name, width x height).
    ListDisplays,
    /// Run until Ctrl+C: capture and POST frames to `ingest_base_url` from config.
    Run {
        /// `data:application/json;base64,...`, raw JSON, or path to a `.json` file.
        #[arg(long)]
        config: String,
    },
}

#[derive(Subcommand, Debug)]
enum GuiCmd {
    /// Build producer `DisplayStreamConfig` JSON for `pirate display-stream run --config` (synonym: screen streaming / «трансляция»).
    Translate {
        /// Producer ingest base URL (e.g. `http://127.0.0.1:39100/ingest`).
        #[arg(long)]
        ingest: Option<String>,
        /// Write JSON to this file instead of stdout.
        #[arg(long)]
        write: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        display_index: u32,
        #[arg(long, default_value_t = 10)]
        fps: u8,
        #[arg(long, default_value_t = 70)]
        quality: u8,
        /// Push monitor list to the server (`ReportDisplayTopology`; requires `pirate auth`).
        #[arg(long)]
        sync_topology: bool,
    },
}

#[derive(Subcommand, Debug)]
enum UninstallTarget {
    /// Remove native server stack (runs `/usr/local/share/pirate-uninstall/uninstall.sh` via sudo).
    Stack {
        /// Only stop services and remove units/binaries/env (keep data and user).
        #[arg(long)]
        services_only: bool,
        /// After uninstall, remove the recorded or default bundle directory (see uninstall.sh).
        #[arg(long)]
        remove_bundle_dir: bool,
        /// Explicit directory to remove (implies `--remove-bundle-dir` with this path).
        #[arg(long, value_name = "PATH")]
        bundle_dir: Option<PathBuf>,
    },
    /// Remove local connection, identity, and settings under the config directory.
    Client,
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
        connection_kind: 0,
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

fn parse_connection_kind_arg(s: &str) -> Result<i32, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "proxy" => Ok(1),
        "resource" => Ok(2),
        "unspecified" | "" => Ok(0),
        _ => Err(format!(
            "unknown connection kind '{s}'; use proxy, resource, or unspecified"
        )),
    }
}

async fn run_profile_cmd(
    cli: &Cli,
    kind: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = resolve_endpoint_normalized(cli);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        eprintln!("endpoint must start with http:// or https://");
        std::process::exit(2);
    }
    if !use_signed_requests(&endpoint) {
        eprintln!("no paired connection for this URL. Run: pirate auth '<install-json>' first");
        std::process::exit(2);
    }
    let kind_i = parse_connection_kind_arg(kind)?;
    let sk = load_or_create_identity()?;
    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
    let mut req = Request::new(UpdateConnectionProfileRequest {
        project_id: cli.project.clone(),
        connection_kind: kind_i,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
    });
    attach_auth_metadata(
        &mut req,
        &sk,
        "UpdateConnectionProfile",
        &cli.project,
        "",
    )?;
    client.update_connection_profile(req).await?;
    if let Some(mut c) = load_connection() {
        c.connection_kind = kind_i;
        save_connection(&c)?;
    }
    println!("connection profile updated (kind={kind_i})");
    Ok(())
}

fn load_display_stream_config(path_or_url: &str) -> Result<DisplayStreamConfig, String> {
    let trimmed = path_or_url.trim();
    let s = if Path::new(trimmed).is_file() {
        std::fs::read_to_string(trimmed).map_err(|e| e.to_string())?
    } else {
        trimmed.to_string()
    };
    DisplayStreamConfig::from_data_url_or_json(&s)
}

async fn run_gui_check_cmd(
    cli: &Cli,
    remote: bool,
    format: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let local = gui_probe::probe_local();
    if !remote {
        if format == "keyval" {
            println!("gui_detected={}", local.gui_detected);
            if let Some(n) = local.monitor_count {
                println!("monitor_count={n}");
            }
            let r = local
                .reasons
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",");
            println!("reasons={r}");
        } else {
            println!("{}", local.to_json_line()?);
        }
        return Ok(());
    }

    let endpoint = resolve_endpoint_normalized(cli);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        eprintln!("endpoint must start with http:// or https://");
        std::process::exit(2);
    }
    if !use_signed_requests(&endpoint) {
        eprintln!("no paired connection for this URL. Run: pirate auth '<install-json>' first");
        std::process::exit(2);
    }
    let sk = load_or_create_identity()?;
    let info = fetch_server_stack_info(&endpoint, Some(&sk)).await?;
    let remote_install = serde_json::json!({
        "host_gui_detected_at_install": info.host_gui_detected_at_install,
        "host_gui_install": info
            .host_gui_install_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
    });
    if format == "keyval" {
        println!("local_gui_detected={}", local.gui_detected);
        println!(
            "remote_host_gui_detected_at_install={}",
            info.host_gui_detected_at_install
                .map(|b| if b { "true" } else { "false" })
                .unwrap_or("unknown")
        );
    } else {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "local".to_string(),
            serde_json::to_value(&local)?,
        );
        obj.insert("remote".to_string(), remote_install);
        println!("{}", serde_json::Value::Object(obj));
    }
    Ok(())
}

async fn run_gui_translate_cmd(
    cli: &Cli,
    ingest: Option<String>,
    write: Option<PathBuf>,
    display_index: u32,
    fps: u8,
    quality: u8,
    sync_topology: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let ingest_url = ingest.as_deref().unwrap_or("http://127.0.0.1:39100/ingest");
    let mut cfg = DisplayStreamConfig::example_producer(ingest_url);
    cfg.display_index = display_index;
    cfg.fps = fps;
    cfg.quality = quality;
    cfg.validate()?;
    let json = cfg.to_json_string()?;
    if let Some(ref p) = write {
        std::fs::write(p, format!("{json}\n"))?;
        eprintln!("wrote {}", p.display());
    } else {
        println!("{json}");
    }
    eprintln!("Run: pirate display-stream run --config <path-or-json>  (or pass JSON string directly)");
    if sync_topology {
        let endpoint = resolve_endpoint_normalized(cli);
        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            eprintln!("endpoint must start with http:// or https://");
            std::process::exit(2);
        }
        if !use_signed_requests(&endpoint) {
            eprintln!("--sync-topology requires prior `pirate auth` for this endpoint");
            std::process::exit(2);
        }
        let sk = load_or_create_identity()?;
        display_stream::send_display_topology(&endpoint, &cli.project, &sk).await?;
        eprintln!("ok: ReportDisplayTopology");
    }
    Ok(())
}

fn try_nvidia_gpu_util_percent() -> Option<f64> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines()
        .next()
        .and_then(|line| line.trim().parse::<f64>().ok())
}

async fn run_resource_report_cmd(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

    let endpoint = resolve_endpoint_normalized(cli);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        eprintln!("endpoint must start with http:// or https://");
        std::process::exit(2);
    }
    if !use_signed_requests(&endpoint) {
        eprintln!("no paired connection for this URL. Run: pirate auth '<install-json>' first");
        std::process::exit(2);
    }
    let sk = load_or_create_identity()?;
    let mut sys = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    sys.refresh_cpu_all();
    std::thread::sleep(Duration::from_millis(200));
    sys.refresh_cpu_usage();
    let cpu_percent = sys.global_cpu_usage() as f64;
    sys.refresh_memory();
    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let ram_percent = if total_mem > 0 {
        100.0 * (used_mem as f64) / (total_mem as f64)
    } else {
        0.0
    };
    let gpu_percent = try_nvidia_gpu_util_percent();

    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
    let mut req = Request::new(ReportResourceUsageRequest {
        project_id: cli.project.clone(),
        cpu_percent: Some(cpu_percent),
        ram_percent: Some(ram_percent),
        gpu_percent,
        ram_used_bytes: Some(used_mem),
        storage_used_bytes: None,
    });
    attach_auth_metadata(
        &mut req,
        &sk,
        "ReportResourceUsage",
        &cli.project,
        "",
    )?;
    client.report_resource_usage(req).await?;
    println!(
        "reported cpu={cpu_percent:.1}% ram={ram_percent:.1}% gpu={}",
        gpu_percent
            .map(|g| format!("{g:.1}%"))
            .unwrap_or_else(|| "n/a".to_string())
    );
    Ok(())
}

fn csv_escape_cell(s: &str) -> String {
    if s.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn print_sessions_unauthenticated_hint(status: &tonic::Status, endpoint: &str) {
    if status.code() != Code::Unauthenticated {
        return;
    }
    let msg = status.message();
    if !msg.contains("x-deploy-pubkey") {
        return;
    }
    eprintln!(
        "hint: run `pirate auth '<install-json>'` (or `pirate pair`) for this host so signed gRPC metadata is sent."
    );
    eprintln!("      Without pair, no x-deploy-pubkey is sent; endpoint was: {endpoint}");
    if let Some(c) = load_connection() {
        let saved = normalize_endpoint(&c.url);
        let want = normalize_endpoint(endpoint);
        if c.paired && !endpoints_equivalent_for_signing(&saved, &want) {
            eprintln!(
                "note: connection.json is paired for URL `{saved}` but this command used `{want}` — omit `--endpoint`, use the same URL, or run `pirate auth` with a bundle for this address."
            );
        } else if !c.paired {
            eprintln!("note: connection.json exists but paired=false; complete `pirate auth` or `pirate pair`.");
        }
    } else {
        eprintln!("note: no saved connection in ~/.config/pirate-client/connection.json for this user.");
        eprintln!("      Run `pirate auth` as the same OS user (avoid `sudo pirate` so ~/.config/pirate-client matches).");
    }
}

async fn run_sessions_cmd(
    cli: &Cli,
    last_log: bool,
    export_log: bool,
    output: Option<&Path>,
    limit: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = resolve_endpoint_normalized(cli);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        eprintln!("endpoint must start with http:// or https://");
        std::process::exit(2);
    }
    let sk = if use_signed_requests(&endpoint) {
        Some(load_or_create_identity()?)
    } else {
        None
    };
    if sk.is_none() {
        match load_connection() {
            Some(c)
                if c.paired
                    && !endpoints_equivalent_for_signing(
                        &normalize_endpoint(&c.url),
                        &normalize_endpoint(&endpoint),
                    ) =>
            {
                let saved = normalize_endpoint(&c.url);
                let want = normalize_endpoint(&endpoint);
                eprintln!(
                    "note: connection.json is paired for `{saved}`; this command uses `{want}` — signed metadata will not be sent. Omit `--endpoint`, use that URL, or re-run `pirate auth` for this host."
                );
            }
            Some(c) if !c.paired => {
                eprintln!(
                    "note: connection.json exists but paired=false; complete `pirate auth` or `pirate pair`."
                );
            }
            None => {
                eprintln!(
                    "note: no ~/.config/pirate-client/connection.json for this user — run `pirate auth '<install-json>'` (avoid `sudo pirate` so the config path matches)."
                );
            }
            _ => {}
        }
    }
    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;

    if export_log {
        let path = output.ok_or("--export-log requires -o/--output PATH")?;
        let mut f = std::fs::File::create(path)?;
        writeln!(
            f,
            "id,created_at_ms,kind,client_public_key_b64,peer_ip,grpc_method,status,detail"
        )?;
        let mut before_id: i64 = 0;
        let page_size = if limit <= 0 { 500 } else { limit.min(500) };
        loop {
            let mut req = Request::new(QuerySessionLogsRequest {
                project_id: cli.project.clone(),
                limit: page_size,
                before_id,
            });
            if let Some(ref k) = sk {
                attach_auth_metadata(&mut req, k, "QuerySessionLogs", &cli.project, "")?;
            }
            let resp = client
                .query_session_logs(req)
                .await
                .map_err(|e| {
                    print_sessions_unauthenticated_hint(&e, &endpoint);
                    e
                })?
                .into_inner();
            if resp.events.is_empty() {
                break;
            }
            let min_id = resp.events.iter().map(|e| e.id).min().unwrap_or(0);
            for e in &resp.events {
                writeln!(
                    f,
                    "{},{},{},{},{},{},{},{}",
                    e.id,
                    e.created_at_ms,
                    csv_escape_cell(&e.kind),
                    csv_escape_cell(&e.client_public_key_b64),
                    csv_escape_cell(&e.peer_ip),
                    csv_escape_cell(&e.grpc_method),
                    csv_escape_cell(&e.status),
                    csv_escape_cell(&e.detail),
                )?;
            }
            if !resp.has_more || min_id <= 0 {
                break;
            }
            before_id = min_id;
        }
        println!("wrote {}", path.display());
        return Ok(());
    }

    if last_log {
        let page = if limit <= 0 { 50 } else { limit.min(500) };
        let mut req = Request::new(QuerySessionLogsRequest {
            project_id: cli.project.clone(),
            limit: page,
            before_id: 0,
        });
        if let Some(ref k) = sk {
            attach_auth_metadata(&mut req, k, "QuerySessionLogs", &cli.project, "")?;
        }
        let resp = client
            .query_session_logs(req)
            .await
            .map_err(|e| {
                print_sessions_unauthenticated_hint(&e, &endpoint);
                e
            })?
            .into_inner();
        println!(
            "{:<12} {:<22} {:<12} {:<10} {:<18} {:<42} {}",
            "id", "created_at_ms", "kind", "status", "peer_ip", "client_pubkey", "grpc_method"
        );
        for e in &resp.events {
            let pk = if e.client_public_key_b64.chars().count() > 40 {
                let mut it = e.client_public_key_b64.chars();
                let head: String = it.by_ref().take(39).collect();
                format!("{head}…")
            } else {
                e.client_public_key_b64.clone()
            };
            println!(
                "{:<12} {:<22} {:<12} {:<10} {:<18} {:<42} {}",
                e.id,
                e.created_at_ms,
                e.kind,
                e.status,
                e.peer_ip,
                pk,
                e.grpc_method
            );
        }
        return Ok(());
    }

    let mut req = Request::new(ListSessionsRequest {
        project_id: cli.project.clone(),
    });
    if let Some(ref k) = sk {
        attach_auth_metadata(&mut req, k, "ListSessions", &cli.project, "")?;
    }
    let resp = client
        .list_sessions(req)
        .await
        .map_err(|e| {
            print_sessions_unauthenticated_hint(&e, &endpoint);
            e
        })?
        .into_inner();
    println!(
        "{:<36} {:>5} {:>4} {:>4} {:>4} {:>8} {:>9} {:<18} {}",
        "client_public_key",
        "kind",
        "cpu%",
        "ram%",
        "gpu%",
        "px_in",
        "px_out",
        "last_ip",
        "method"
    );
    for p in &resp.peers {
        let pk = truncate_pubkey_display(&p.client_public_key_b64, 35);
        let cpu = p
            .last_cpu_percent
            .map(|x| format!("{x:.0}"))
            .unwrap_or_else(|| "—".to_string());
        let ram = p
            .last_ram_percent
            .map(|x| format!("{x:.0}"))
            .unwrap_or_else(|| "—".to_string());
        let gpu = p
            .last_gpu_percent
            .map(|x| format!("{x:.0}"))
            .unwrap_or_else(|| "—".to_string());
        println!(
            "{:<36} {:>5} {:>4} {:>4} {:>4} {:>8} {:>9} {:<18} {}",
            pk,
            p.connection_kind,
            cpu,
            ram,
            gpu,
            p.proxy_bytes_in_total,
            p.proxy_bytes_out_total,
            p.last_peer_ip,
            p.last_grpc_method,
        );
    }
    Ok(())
}

fn truncate_pubkey_display(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if cli.version_all {
        version_info::run_version_all(&cli.endpoint).await?;
        return Ok(());
    }

    if cli.version {
        version_info::run_version(&cli.endpoint).await?;
        return Ok(());
    }

    let Some(command) = cli.command.as_ref() else {
        Cli::command().print_help()?;
        return Ok(());
    };

    match command {
        Commands::ShowPubkey { identity } => {
            let path = match identity {
                Some(p) => p.clone(),
                None => identity_path()
                    .ok_or("no config directory; set XDG_CONFIG_HOME or use --identity")?,
            };
            let sk = load_identity(&path)?;
            println!("{}", pubkey_b64_url(&sk));
            return Ok(());
        }
        Commands::Auth { bundle } => {
            run_auth(bundle.clone(), cli.project.clone()).await?;
            return Ok(());
        }
        Commands::Board {
            listen,
            test_connect,
            probe_json,
            probe_upload_bytes,
            probe_download_bytes,
            board,
            session_token,
            settings,
        } => {
            let url = resolve_endpoint_normalized(&cli);
            if !url.starts_with("http://") && !url.starts_with("https://") {
                eprintln!("URL must start with http:// or https://");
                std::process::exit(2);
            }
            if !use_signed_requests(&url) {
                eprintln!(
                    "no paired connection for this URL. Run: pirate auth '<install-json>' first"
                );
                std::process::exit(2);
            }
            if *probe_json && !*test_connect {
                eprintln!("--probe-json requires --test-connect");
                std::process::exit(2);
            }
            let sk = load_or_create_identity()?;
            if *test_connect {
                board_probe::run_board_test_connect(
                    url.as_str(),
                    &cli.project,
                    &sk,
                    *probe_upload_bytes,
                    *probe_download_bytes,
                    *probe_json,
                )
                .await?;
            } else {
                let settings_path = settings
                    .clone()
                    .or_else(settings_path)
                    .unwrap_or_else(|| PathBuf::from("settings.json"));
                let snap = settings::init_global_settings(settings_path)?;
                let pool = std::sync::Arc::new(ConnectionManager::new(512));
                let conn_url = load_connection()
                    .map(|c| c.url)
                    .unwrap_or_else(|| url.clone());
                board::run_board(
                    listen,
                    url.as_str(),
                    conn_url.as_str(),
                    &sk,
                    &cli.project,
                    board.as_deref().unwrap_or(""),
                    snap,
                    pool,
                    session_token.as_deref(),
                    None,
                )
                .await?;
            }
            return Ok(());
        }
        Commands::TestProxy {
            speed,
            max_connect,
            proxy,
            upstream_url,
            bytes,
            timeout,
            cap,
            min_success_rate,
            json,
        } => {
            use deploy_client::proxy_test::{self, TestProxyOptions};
            let upstream = upstream_url
                .clone()
                .unwrap_or_else(proxy_test::default_upstream_url);
            let opts = TestProxyOptions {
                proxy_url: proxy.clone(),
                upstream_url: upstream,
                bytes: *bytes,
                timeout_secs: *timeout,
                max_connect_cap: *cap,
                min_success_rate: *min_success_rate,
                run_speed: *speed,
                run_max_connect: *max_connect,
                json: *json,
            };
            proxy_test::run_proxy_tests(opts).await?;
            return Ok(());
        }
        Commands::BootstrapApply {
            file,
            url,
            session_token,
            settings,
        } => {
            use bootstrap_apply::{apply_bootstrap_json, load_json_from_file_or_url};
            if file.is_none() && url.is_none() {
                eprintln!("provide --file PATH or --url URL");
                std::process::exit(2);
            }
            let json = load_json_from_file_or_url(file.as_deref(), url.as_deref()).await?;
            let settings_path = settings
                .clone()
                .or_else(settings_path)
                .unwrap_or_else(|| PathBuf::from("settings.json"));
            apply_bootstrap_json(&json, session_token.as_deref(), &settings_path)?;
            println!("Updated {}", settings_path.display());
            return Ok(());
        }
        Commands::Pair { bundle } => {
            run_pair(bundle.clone()).await?;
            return Ok(());
        }
        Commands::Sessions {
            last_log,
            export_log,
            output,
            limit,
        } => {
            if *export_log && output.is_none() {
                eprintln!("--export-log requires -o/--output PATH");
                std::process::exit(2);
            }
            if *last_log && *export_log {
                eprintln!("use either --last-log or --export-log, not both");
                std::process::exit(2);
            }
            run_sessions_cmd(&cli, *last_log, *export_log, output.as_deref(), *limit).await?;
            return Ok(());
        }
        Commands::Profile { kind } => {
            run_profile_cmd(&cli, kind).await?;
            return Ok(());
        }
        Commands::ResourceReport => {
            run_resource_report_cmd(&cli).await?;
            return Ok(());
        }
        Commands::DisplayStream { cmd } => {
            match cmd {
                DisplayStreamCmd::ListDisplays => {
                    display_stream::list_displays()?;
                }
                DisplayStreamCmd::Run { config } => {
                    let cfg = load_display_stream_config(config)?;
                    let endpoint = resolve_endpoint_normalized(&cli);
                    let topo =
                        if use_signed_requests(&endpoint)
                            && (endpoint.starts_with("http://") || endpoint.starts_with("https://"))
                        {
                            let sk = load_or_create_identity()?;
                            Some((endpoint, cli.project.clone(), sk))
                        } else {
                            None
                        };
                    display_stream::run_producer(cfg, topo).await?;
                }
            }
            return Ok(());
        }
        Commands::GuiCheck { remote, format } => {
            run_gui_check_cmd(&cli, *remote, format.as_str()).await?;
            return Ok(());
        }
        Commands::Gui { cmd } => {
            match cmd {
                GuiCmd::Translate {
                    ingest,
                    write,
                    display_index,
                    fps,
                    quality,
                    sync_topology,
                } => {
                    run_gui_translate_cmd(
                        &cli,
                        ingest.clone(),
                        write.clone(),
                        *display_index,
                        *fps,
                        *quality,
                        *sync_topology,
                    )
                    .await?;
                }
            }
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
        Commands::Uninstall { target } => {
            match target {
                UninstallTarget::Stack {
                    services_only,
                    remove_bundle_dir,
                    bundle_dir,
                } => {
                    deploy_client::local_uninstall::run_uninstall_stack(
                        *services_only,
                        *remove_bundle_dir,
                        bundle_dir.as_deref(),
                    )?;
                }
                UninstallTarget::Client => {
                    deploy_client::local_uninstall::remove_local_client_config()?;
                    eprintln!(
                        "Removed local Pirate client config (connection, identity, settings)."
                    );
                    eprintln!(
                        "If `pirate board` or similar is still running, stop it manually (no PID file is used)."
                    );
                }
            }
            return Ok(());
        }
        Commands::InitProject { path, name } => {
            let p = deploy_client::init_project(path.as_path(), name.as_deref())?;
            println!("{}", p.display());
            return Ok(());
        }
        Commands::ScanProject { path, dry_run } => {
            let r = deploy_client::scan_project(path.as_path(), *dry_run)?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            return Ok(());
        }
        Commands::ProjectBuild { path } => {
            let r = deploy_client::run_build(path.as_path())?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            if !r.ok {
                std::process::exit(1);
            }
            return Ok(());
        }
        Commands::ProjectTest { path } => {
            let r = deploy_client::run_test(path.as_path())?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            if !r.ok {
                std::process::exit(1);
            }
            return Ok(());
        }
        Commands::TestLocal { path, image } => {
            let r = deploy_client::test_local_docker(path.as_path(), image.as_str())?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            if !r.ok {
                std::process::exit(1);
            }
            return Ok(());
        }
        Commands::ApplyGen { path } => {
            deploy_client::apply_generated_files(path.as_path())?;
            println!("ok: generated run.sh / sidecars under {}", path.display());
            return Ok(());
        }
        Commands::Project { name, cmd } => {
            let root = deploy_client::resolve_path(name)?;
            match cmd {
                ProjectCmd::Build => {
                    let r = deploy_client::run_build(root.as_path())?;
                    println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                    if !r.ok {
                        std::process::exit(1);
                    }
                }
                ProjectCmd::Test => {
                    let r = deploy_client::run_test(root.as_path())?;
                    println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                    if !r.ok {
                        std::process::exit(1);
                    }
                }
                ProjectCmd::Deploy {
                    message,
                    release,
                    chunk_size,
                } => {
                    if let Some(ref m) = message {
                        eprintln!("deploy message: {}", m);
                        let dir = root.join(".pirate");
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(dir.join("last-deploy-message.txt"), m);
                    }
                    let endpoint = resolve_endpoint_normalized(&cli);
                    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                        eprintln!("endpoint must start with http:// or https://");
                        std::process::exit(2);
                    }
                    let version = release.clone().unwrap_or_else(default_version);
                    validate_version_label(&version)?;
                    eprintln!("packing {} …", root.display());
                    let sk = if use_signed_requests(&endpoint) {
                        Some(load_or_create_identity()?)
                    } else {
                        None
                    };
                    let resp = deploy_directory(
                        &endpoint,
                        root.as_path(),
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
            }
            return Ok(());
        }
        Commands::Projects { sub } => {
            match sub {
                ProjectsCmd::Add { path } => {
                    let n = deploy_client::register_from_pirate_toml_dir(path.as_path())?;
                    println!("registered: {}", n);
                }
                ProjectsCmd::List => {
                    let m = deploy_client::list_projects()?;
                    println!("{}", serde_json::to_string_pretty(&m).unwrap_or_default());
                }
                ProjectsCmd::Remove { name } => {
                    if !deploy_client::remove_project_registry(name)? {
                        eprintln!("no project named {:?} in registry", name);
                        std::process::exit(1);
                    }
                }
            }
            return Ok(());
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

            let sk = if use_signed_requests(&endpoint) {
                Some(load_or_create_identity()?)
            } else {
                None
            };

            let sk_info = fetch_server_stack_info(&endpoint, sk.as_ref())
                .await
                .map_err(|e| format!("GetServerStackInfo: {}", e.message()))?;

            let bundle_profile = inspect_bundle_path(path.as_path())
                .map_err(|e| format!("inspect bundle {}: {e}", path.display()))?;
            let apply_options = stack_update_prompt::resolve_stack_apply_options(
                sk_info.host_dashboard_enabled,
                &bundle_profile,
            )?;

            eprintln!("reading {} …", path.display());
            let artifact = read_or_pack_bundle(path.as_path()).map_err(|e| {
                format!("cannot read bundle {}: {e}", path.display())
            })?;
            let total = artifact.len() as u64;
            eprintln!("artifact_bytes={total}");

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
                apply_options,
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
