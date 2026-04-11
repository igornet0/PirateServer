use deploy_proto::deploy::DeployChunk;
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
