use deploy_proto::deploy::{DeployChunk, ServerStackChunk};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::path::Path;
use tar::Builder;

pub fn default_version() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("v-{ts}")
}

pub fn validate_version(version: &str) -> Result<(), std::io::Error> {
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

pub fn pack_directory(dir: &Path) -> Result<Vec<u8>, std::io::Error> {
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

pub fn build_chunks(
    bytes: &[u8],
    version: &str,
    project_id: &str,
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
            project_id: project_id.to_string(),
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
            project_id: if first {
                project_id.to_string()
            } else {
                String::new()
            },
        });
        first = false;
        offset = end;
    }
    out
}

/// Chunks for `UploadServerStack` (no `project_id`).
pub fn build_server_stack_chunks(
    bytes: &[u8],
    version: &str,
    sha256_hex: &str,
    chunk_size: usize,
) -> Vec<ServerStackChunk> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    if bytes.is_empty() {
        return vec![ServerStackChunk {
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
        out.push(ServerStackChunk {
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

/// Pre-built `.tar.gz` / `.tgz` or a directory to pack (same layout as `build-linux-bundle.sh`).
pub fn read_or_pack_bundle(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    if path.is_file() {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            return std::fs::read(path);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bundle file must be .tar.gz or .tgz",
        ));
    }
    if path.is_dir() {
        return pack_directory(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "bundle path not found",
    ))
}
