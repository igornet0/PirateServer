use deploy_proto::deploy::{DeployChunk, ServerStackChunk, StackApplyOptions};
use flate2::write::GzEncoder;
use flate2::Compression;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
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

fn path_to_unix_rel(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn compile_ignore_globs(ignore_path: Option<&Path>) -> Result<Option<GlobSet>, std::io::Error> {
    let Some(path) = ignore_path else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    let mut b = GlobSetBuilder::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with('!') {
            // Start with plain exclusion semantics; exceptions may be added later.
            continue;
        }
        let glob = Glob::new(t).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid .pirateignore pattern `{t}`: {e}"),
            )
        })?;
        b.add(glob);
    }
    let set = b.build().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid .pirateignore: {e}"),
        )
    })?;
    Ok(Some(set))
}

fn output_paths_or_default(outputs: &[String]) -> Vec<String> {
    if outputs.is_empty() {
        vec![".".to_string()]
    } else {
        outputs.to_vec()
    }
}

fn resolve_output_path(project_root: &Path, rel: &str) -> Result<PathBuf, std::io::Error> {
    if rel.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "build output path must not be empty",
        ));
    }
    let p = Path::new(rel.trim());
    if p.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("build output path must be relative: {rel}"),
        ));
    }
    let joined = project_root.join(p);
    let can = joined.canonicalize().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("build output path not found `{rel}`: {e}"),
        )
    })?;
    let root = project_root.canonicalize()?;
    if !can.starts_with(&root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("build output path escapes project root: {rel}"),
        ));
    }
    Ok(can)
}

fn collect_files_recursive(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    for ent in fs::read_dir(root)? {
        let ent = ent?;
        let p = ent.path();
        let meta = ent.metadata()?;
        if meta.is_dir() {
            collect_files_recursive(&p, out)?;
        } else if meta.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

/// Write release tarball to `out_path` (`.tar.gz`). Same layout as [`pack_release_sources`].
pub fn pack_release_sources_to_path(
    project_root: &Path,
    output_paths: &[String],
    ignore_path: Option<&Path>,
    out_path: &Path,
) -> Result<(), std::io::Error> {
    let project_root = project_root.canonicalize()?;
    let ignore = compile_ignore_globs(ignore_path)?;
    let requested = output_paths_or_default(output_paths);
    let mut files: Vec<PathBuf> = Vec::new();
    for rel in requested {
        let resolved = resolve_output_path(&project_root, &rel)?;
        let meta = fs::metadata(&resolved)?;
        if meta.is_file() {
            files.push(resolved);
        } else if meta.is_dir() {
            collect_files_recursive(&resolved, &mut files)?;
        }
    }
    let mut uniq = BTreeSet::<String>::new();
    let raw = fs::File::create(out_path)?;
    let enc = GzEncoder::new(raw, Compression::default());
    let mut builder = Builder::new(enc);
    for f in files {
        let rel = f
            .strip_prefix(&project_root)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let rel_unix = path_to_unix_rel(rel);
        if rel_unix.is_empty() || rel_unix == "." {
            continue;
        }
        if let Some(ref set) = ignore {
            if set.is_match(rel_unix.as_str()) {
                continue;
            }
        }
        if !uniq.insert(rel_unix.clone()) {
            continue;
        }
        builder
            .append_path_with_name(&f, rel)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    }
    builder
        .finish()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let enc = builder
        .into_inner()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    enc.finish()?;
    Ok(())
}

pub fn pack_release_sources(
    project_root: &Path,
    output_paths: &[String],
    ignore_path: Option<&Path>,
) -> Result<Vec<u8>, std::io::Error> {
    let tmp = std::env::temp_dir().join(format!(
        "pirate-pack-{}-{}.tar.gz",
        std::process::id(),
        rand::random::<u64>()
    ));
    pack_release_sources_to_path(project_root, output_paths, ignore_path, &tmp)?;
    let out = fs::read(&tmp)?;
    let _ = fs::remove_file(&tmp);
    Ok(out)
}

pub fn pack_directory(dir: &Path) -> Result<Vec<u8>, std::io::Error> {
    pack_release_sources(dir, &[], Some(&dir.join(".pirateignore")))
}

pub fn build_chunks(
    bytes: &[u8],
    version: &str,
    project_id: &str,
    sha256_hex: &str,
    chunk_size: usize,
) -> Vec<DeployChunk> {
    build_chunks_with_manifest(bytes, version, project_id, sha256_hex, chunk_size, None, "tar_gz")
}

/// Same as [`build_chunks`], optional `pirate.toml` body on last chunk + artifact kind label.
pub fn build_chunks_with_manifest(
    bytes: &[u8],
    version: &str,
    project_id: &str,
    sha256_hex: &str,
    chunk_size: usize,
    manifest_toml: Option<&str>,
    artifact_kind: &str,
) -> Vec<DeployChunk> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    let man = manifest_toml.unwrap_or("");
    let kind = artifact_kind.to_string();
    if bytes.is_empty() {
        return vec![DeployChunk {
            data: vec![],
            version: version.to_string(),
            is_last: true,
            sha256_hex: sha256_hex.to_string(),
            project_id: project_id.to_string(),
            manifest_toml: man.to_string(),
            artifact_kind: kind,
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
            manifest_toml: if is_last {
                man.to_string()
            } else {
                String::new()
            },
            artifact_kind: if is_last {
                kind.clone()
            } else {
                String::new()
            },
        });
        first = false;
        offset = end;
    }
    out
}

/// Chunks for `UploadServerStack` (no `project_id`). `apply_options` is attached only to the last chunk.
pub fn build_server_stack_chunks(
    bytes: &[u8],
    version: &str,
    sha256_hex: &str,
    chunk_size: usize,
    apply_options: Option<StackApplyOptions>,
) -> Vec<ServerStackChunk> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    if bytes.is_empty() {
        return vec![ServerStackChunk {
            data: vec![],
            version: version.to_string(),
            is_last: true,
            sha256_hex: sha256_hex.to_string(),
            apply_options,
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
            apply_options: if is_last { apply_options.clone() } else { None },
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
