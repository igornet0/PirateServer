//! CLI: pack `build/` as tar.gz, stream over gRPC (IPv6 endpoint).

use clap::{Parser, Subcommand};
use deploy_proto::deploy::{DeployChunk, RollbackRequest, StatusRequest};
use deploy_proto::DeployServiceClient;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::stream;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tar::Builder;
use tonic::Request;

/// Default gRPC HTTP/2 endpoint (IPv6 loopback).
const DEFAULT_ENDPOINT: &str = "http://[::1]:50051";

#[derive(Parser, Debug)]
#[command(
    name = "client",
    about = "Deploy artifact to deploy-server over gRPC (IPv6)"
)]
struct Cli {
    /// Server endpoint, e.g. http://[::1]:50051 or http://[2001:db8::1]:50051
    #[arg(long, default_value = DEFAULT_ENDPOINT, global = true)]
    endpoint: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
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

fn default_version() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("v-{ts}")
}

fn validate_version(version: &str) -> Result<(), std::io::Error> {
    if version.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "version must not be empty",
        ));
    }
    if version.len() > 128 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "version too long",
        ));
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "version may only contain [a-zA-Z0-9._-]",
        ));
    }
    if version.contains("..") || version.contains('/') || version.contains('\\') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid version string",
        ));
    }
    Ok(())
}

fn pack_directory(dir: &Path) -> Result<Vec<u8>, std::io::Error> {
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(enc);
    builder
        .append_dir_all(".", dir)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    builder
        .finish()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let enc = builder
        .into_inner()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let out = enc.finish()?;
    Ok(out)
}

fn build_chunks(
    bytes: &[u8],
    version: &str,
    sha256_hex: &str,
    chunk_size: usize,
) -> Vec<DeployChunk> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    if bytes.is_empty() {
        return vec![DeployChunk {
            data: vec![],
            version: version.to_string(),
            is_last: true,
            sha256_hex: sha256_hex.to_string(),
        }];
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut first = true;
    while offset < bytes.len() {
        let end = (offset + chunk_size).min(bytes.len());
        let data = bytes[offset..end].to_vec();
        let is_last = end >= bytes.len();
        out.push(DeployChunk {
            data,
            version: if first {
                version.to_string()
            } else {
                String::new()
            },
            is_last,
            sha256_hex: if is_last {
                sha256_hex.to_string()
            } else {
                String::new()
            },
        });
        first = false;
        offset = end;
    }
    out
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let endpoint = cli.endpoint.trim().to_string();
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        eprintln!("endpoint must start with http:// or https:// (e.g. {DEFAULT_ENDPOINT})");
        std::process::exit(2);
    }

    match cli.command {
        Commands::Deploy {
            path,
            version,
            chunk_size,
        } => {
            let dir = path.canonicalize().map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("cannot resolve {}: {e}", path.display()),
                )
            })?;
            if !dir.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("not a directory: {}", dir.display()),
                )
                .into());
            }

            let version = version.unwrap_or_else(default_version);
            validate_version(&version)?;

            eprintln!("packing {} …", dir.display());
            let artifact = pack_directory(&dir)?;
            let digest = Sha256::digest(&artifact);
            let sha256_hex = hex::encode(digest);

            let chunks = build_chunks(&artifact, &version, &sha256_hex, chunk_size);
            eprintln!(
                "uploading {} bytes in {} chunk(s) as version {version} …",
                artifact.len(),
                chunks.len()
            );

            let mut client = DeployServiceClient::connect(endpoint).await?;
            let stream = stream::iter(chunks);
            let resp = client.upload(Request::new(stream)).await?.into_inner();
            println!(
                "status={} deployed_version={}",
                resp.status, resp.deployed_version
            );
        }
        Commands::Status => {
            let mut client = DeployServiceClient::connect(endpoint).await?;
            let r = client
                .get_status(Request::new(StatusRequest {}))
                .await?
                .into_inner();
            println!("current_version={} state={}", r.current_version, r.state);
        }
        Commands::Rollback { version } => {
            validate_version(&version)?;
            let mut client = DeployServiceClient::connect(endpoint).await?;
            let r = client
                .rollback(Request::new(RollbackRequest { version }))
                .await?
                .into_inner();
            println!("status={} active_version={}", r.status, r.active_version);
        }
    }

    Ok(())
}
