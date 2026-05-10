//! Pirate file storage (control-api): user-visible files under a dedicated root, quota, path safety.

use deploy_db::DbStore;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const RESERVED_TMP: &str = ".pirate-tmp";
const MAX_PATH_COMPONENTS: usize = 64;
const MAX_NAME_BYTES: usize = 255;

/// How to handle existing paths during archive extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageExtractConflictMode {
    /// Stop and return `ExtractConflict` on first existing path.
    Abort,
    /// Overwrite files; skip creating a file if a directory is in the way (still conflict).
    Overwrite,
    /// Remove file or whole directory at conflict path, then write.
    DeleteAndOverwrite,
}

/// Outcome of `extract_archive` for JSON responses.
#[derive(Debug, Clone, Serialize)]
pub struct StorageExtractView {
    pub ok: bool,
    pub extracted_files: u64,
    pub created_dirs: u64,
    pub skipped: u64,
    pub warnings: Vec<String>,
}

/// Configuration for the file storage area (independent of project deploy state).
#[derive(Debug, Clone)]
pub struct PirateStorageConfig {
    pub root: PathBuf,
    /// 0 = unlimited
    pub max_bytes: u64,
    /// Max single file upload; should be aligned with control-api `DefaultBodyLimit` and env.
    pub max_upload_bytes: u64,
}

#[derive(Debug, Error)]
pub enum PirateStorageError {
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("path already exists: {0}")]
    AlreadyExists(String),
    /// Extraction would overwrite; client should retry with overwrite or delete_and_overwrite.
    #[error("extract conflict: {path}")]
    ExtractConflict { path: String },
    #[error("unsupported archive format: {0}")]
    UnsupportedArchive(String),
    #[error("corrupt or unreadable archive: {0}")]
    CorruptArchive(String),
    #[error("storage quota: used would be {used}; max {max}")]
    Quota { used: u64, max: u64, need: u64 },
    #[error("name too long: {0}")]
    NameTooLong(String),
    #[error("storage is not configured (set PIRATE_STORAGE_ROOT)")]
    NotConfigured,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("database: {0}")]
    Db(String),
}

/// Relative path, POSIX style, no leading slash, no `..` segments.
pub fn normalize_rel_path(raw: &str) -> Result<String, PirateStorageError> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if t.chars().any(|c| c == '\0' || c == '\\') {
        return Err(PirateStorageError::InvalidPath(
            "NUL or backslash not allowed".to_string(),
        ));
    }
    let mut out: Vec<&str> = Vec::new();
    for p in t.split('/') {
        if p.is_empty() {
            continue;
        }
        if p == ".." {
            return Err(PirateStorageError::InvalidPath(
                "parent path segments not allowed".to_string(),
            ));
        }
        if p == "." {
            return Err(PirateStorageError::InvalidPath("'.' segment not allowed".to_string()));
        }
        if p.as_bytes().len() > MAX_NAME_BYTES {
            return Err(PirateStorageError::NameTooLong(p.to_string()));
        }
        if p == RESERVED_TMP {
            return Err(PirateStorageError::InvalidPath("reserved name".to_string()));
        }
        out.push(p);
    }
    if out.len() > MAX_PATH_COMPONENTS {
        return Err(PirateStorageError::InvalidPath("path too deep".to_string()));
    }
    Ok(out.join("/"))
}

/// Resolve a logical path under `root` without creating directories (no IO except canonicalizing `root` once).
pub fn resolve_path(root: &Path, rel: &str) -> Result<PathBuf, PirateStorageError> {
    let s = normalize_rel_path(rel)?;
    let root: PathBuf = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    if s.is_empty() {
        return Ok(root);
    }
    let mut p = root.clone();
    for part in s.split('/') {
        p.push(part);
    }
    if !p.starts_with(&root) {
        return Err(PirateStorageError::InvalidPath("path escapes root".to_string()));
    }
    Ok(p)
}

/// `resolve_path` then require path exists. Used for read/delete; follows symlinks and re-checks containment.
pub fn resolve_existing_path(root: &Path, rel: &str) -> Result<PathBuf, PirateStorageError> {
    let p = resolve_path(root, rel)?;
    if !p.exists() {
        return Err(PirateStorageError::NotFound(rel.to_string()));
    }
    let root: PathBuf = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let canon = p.canonicalize()?;
    if !canon.starts_with(&root) {
        return Err(PirateStorageError::InvalidPath("path escapes root".to_string()));
    }
    Ok(canon)
}

/// Sum file sizes in `root` excluding `root/.pirate-tmp/`.
pub fn walk_storage_used_bytes(root: &Path) -> u64 {
    let skip = root.join(RESERVED_TMP);
    let mut total: u64 = 0;
    fn walk(
        cur: &Path,
        root: &Path,
        skip: &Path,
        total: &mut u64,
    ) -> io::Result<()> {
        for e in fs::read_dir(cur)? {
            let e = e?;
            let p = e.path();
            if p == skip || p.starts_with(skip) {
                continue;
            }
            if p == root {
                // continue into children; don't count root
            }
            let meta = e.metadata()?;
            if meta.is_file() {
                *total = total.saturating_add(meta.len());
            } else if meta.is_dir() {
                walk(&p, root, skip, total)?;
            }
        }
        Ok(())
    }
    if root.is_dir() {
        if let Err(err) = walk(root, root, &skip, &mut total) {
            tracing::warn!(%err, "storage walk: partial result");
        }
    }
    total
}

fn ensure_configured(cfg: &PirateStorageConfig) -> Result<(), PirateStorageError> {
    if !cfg.root.is_dir() {
        return Err(PirateStorageError::NotConfigured);
    }
    Ok(())
}

async fn maybe_reconcile_db(db: &DbStore, root: &Path) -> Result<u64, PirateStorageError> {
    let used = walk_storage_used_bytes(root);
    db.pirate_file_storage_set_used_bytes(used)
        .await
        .map_err(|e| PirateStorageError::Db(e.to_string()))?;
    Ok(used)
}

/// Returns used bytes, reconciling the DB row from the filesystem.
pub async fn storage_usage(
    db: &DbStore,
    root: &Path,
    max_bytes: u64,
) -> Result<StorageUsageView, PirateStorageError> {
    if !root.is_dir() {
        return Err(PirateStorageError::NotConfigured);
    }
    let used = maybe_reconcile_db(db, root).await?;
    build_usage(used, max_bytes)
}

/// Usage when control-api has no DB (counters are derived from a FS walk on each call).
pub fn storage_usage_no_db(root: &Path, max_bytes: u64) -> Result<StorageUsageView, PirateStorageError> {
    if !root.is_dir() {
        return Err(PirateStorageError::NotConfigured);
    }
    let used = walk_storage_used_bytes(root);
    build_usage(used, max_bytes)
}

fn build_usage(used: u64, max_bytes: u64) -> Result<StorageUsageView, PirateStorageError> {
    Ok(StorageUsageView {
        used_bytes: used,
        max_bytes,
        free_bytes: if max_bytes == 0 {
            None
        } else {
            Some(max_bytes.saturating_sub(used))
        },
        used_percent: if max_bytes == 0 {
            None
        } else {
            Some((used as f64 * 100.0 / (max_bytes as f64)) as f32)
        },
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageUsageView {
    pub used_bytes: u64,
    pub max_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageListView {
    /// Parent directory relative path
    pub path: String,
    pub entries: Vec<StorageEntryView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageEntryView {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub mtime_ms: i64,
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn rel_from_root(root: &Path, full: &Path) -> String {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let full = full
        .canonicalize()
        .unwrap_or_else(|_| full.to_path_buf());
    full.strip_prefix(&root)
        .ok()
        .and_then(|p| p.to_str())
        .map(|s| {
            s.trim()
                .trim_start_matches(std::path::MAIN_SEPARATOR)
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// List a single directory.
pub async fn list_tree(
    cfg: &PirateStorageConfig,
    rel_parent: &str,
) -> Result<StorageListView, PirateStorageError> {
    ensure_configured(cfg)?;
    let rel = normalize_rel_path(rel_parent)?;
    let parent = resolve_path(&cfg.root, &rel)?;
    if !parent.is_dir() {
        return Err(PirateStorageError::NotFound(rel));
    }
    let mut out = Vec::new();
    let mut rd = tokio::fs::read_dir(&parent)
        .await
        .map_err(PirateStorageError::Io)?;
    while let Some(e) = rd
        .next_entry()
        .await
        .map_err(PirateStorageError::Io)?
    {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == RESERVED_TMP {
            continue;
        }
        let full = e.path();
        let meta = match e.metadata().await {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %full.display(),
                    "storage list_tree: skip directory entry (metadata)"
                );
                continue;
            }
        };
        let rel_path = rel_from_root(&cfg.root, &full);
        let kind = if meta.is_dir() { "dir" } else { "file" };
        let size = if meta.is_dir() { 0u64 } else { meta.len() };
        out.push(StorageEntryView {
            name: name.to_string(),
            path: rel_path,
            kind: kind.to_string(),
            size,
            mtime_ms: mtime_ms(&meta),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(StorageListView { path: rel, entries: out })
}

/// Create directory (including parents).
pub async fn create_folder(cfg: &PirateStorageConfig, rel: &str) -> Result<(), PirateStorageError> {
    ensure_configured(cfg)?;
    if normalize_rel_path(rel)?.is_empty() {
        return Err(PirateStorageError::InvalidPath(
            "empty path not allowed for folder create".to_string(),
        ));
    }
    let path = resolve_path(&cfg.root, rel)?;
    if path.exists() {
        return Err(PirateStorageError::AlreadyExists(rel.to_string()));
    }
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(PirateStorageError::Io)
}

pub async fn remove_file(
    cfg: &PirateStorageConfig,
    db: Option<&DbStore>,
    rel: &str,
) -> Result<(), PirateStorageError> {
    ensure_configured(cfg)?;
    let p = resolve_existing_path(&cfg.root, rel)?;
    if !p.is_file() {
        if p.is_dir() {
            return Err(PirateStorageError::InvalidPath(
                "expected a file path".to_string(),
            ));
        }
        return Err(PirateStorageError::NotFound(rel.to_string()));
    }
    tokio::fs::remove_file(&p)
        .await
        .map_err(PirateStorageError::Io)?;
    if let Some(d) = db {
        let _ = maybe_reconcile_db(d, &cfg.root).await?;
    }
    Ok(())
}

/// Remove directory, optionally recursive.
pub async fn remove_dir(
    cfg: &PirateStorageConfig,
    db: Option<&DbStore>,
    rel: &str,
    recursive: bool,
) -> Result<(), PirateStorageError> {
    ensure_configured(cfg)?;
    if normalize_rel_path(rel)?.is_empty() {
        return Err(PirateStorageError::InvalidPath(
            "cannot remove storage root".to_string(),
        ));
    }
    let p = resolve_existing_path(&cfg.root, rel)?;
    if !p.is_dir() {
        if p.is_file() {
            return Err(PirateStorageError::InvalidPath(
                "expected a directory path".to_string(),
            ));
        }
        return Err(PirateStorageError::NotFound(rel.to_string()));
    }
    if recursive {
        tokio::fs::remove_dir_all(&p)
            .await
            .map_err(PirateStorageError::Io)?;
    } else {
        let mut e = tokio::fs::read_dir(&p)
            .await
            .map_err(PirateStorageError::Io)?;
        if e
            .next_entry()
            .await
            .map_err(PirateStorageError::Io)?
            .is_some()
        {
            return Err(PirateStorageError::InvalidPath(
                "directory not empty (use recursive)".to_string(),
            ));
        }
        tokio::fs::remove_dir(&p)
            .await
            .map_err(PirateStorageError::Io)?;
    }
    if let Some(d) = db {
        let _ = maybe_reconcile_db(d, &cfg.root).await?;
    }
    Ok(())
}

/// From / to, both relative paths. Same directory rename or move.
pub async fn rename_path(
    cfg: &PirateStorageConfig,
    db: Option<&DbStore>,
    from_rel: &str,
    to_rel: &str,
) -> Result<(), PirateStorageError> {
    ensure_configured(cfg)?;
    let a = resolve_existing_path(&cfg.root, from_rel)?;
    let b = resolve_path(&cfg.root, to_rel)?;
    if b.exists() {
        return Err(PirateStorageError::AlreadyExists(to_rel.to_string()));
    }
    if let Some(p) = b.parent() {
        tokio::fs::create_dir_all(p)
            .await
            .map_err(PirateStorageError::Io)?;
    }
    tokio::fs::rename(&a, &b)
        .await
        .map_err(PirateStorageError::Io)?;
    if let Some(d) = db {
        let _ = maybe_reconcile_db(d, &cfg.root).await?;
    }
    Ok(())
}

/// Write full body to a temp file, then `rename` to the final path.
pub async fn store_uploaded_file(
    cfg: &PirateStorageConfig,
    db: Option<&DbStore>,
    rel_path: &str,
    data: Vec<u8>,
) -> Result<u64, PirateStorageError> {
    ensure_configured(cfg)?;
    let n = data.len() as u64;
    if n > cfg.max_upload_bytes {
        return Err(PirateStorageError::InvalidPath(
            "file exceeds PIRATE_STORAGE_MAX_UPLOAD_BYTES".to_string(),
        ));
    }
    if normalize_rel_path(rel_path)?.is_empty() {
        return Err(PirateStorageError::InvalidPath(
            "path required for file upload".to_string(),
        ));
    }
    let final_path = resolve_path(&cfg.root, rel_path)?;
    if final_path.is_dir() {
        return Err(PirateStorageError::InvalidPath(
            "path is a directory".to_string(),
        ));
    }
    let old: u64 = if final_path.is_file() {
        final_path
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        0
    };
    if cfg.max_bytes > 0 {
        let used = walk_storage_used_bytes(&cfg.root);
        let new_total = used.saturating_sub(old).saturating_add(n);
        if new_total > cfg.max_bytes {
            let need = new_total - cfg.max_bytes;
            return Err(PirateStorageError::Quota {
                used: new_total,
                max: cfg.max_bytes,
                need,
            });
        }
    }
    if let Some(p) = final_path.parent() {
        tokio::fs::create_dir_all(p)
            .await
            .map_err(PirateStorageError::Io)?;
    }
    let tmp_dir = cfg.root.join(RESERVED_TMP);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(PirateStorageError::Io)?;
    let tmp = tmp_dir.join(Uuid::new_v4().to_string());
    {
        let mut f = tokio::fs::File::create(&tmp)
            .await
            .map_err(PirateStorageError::Io)?;
        f.write_all(&data)
            .await
            .map_err(PirateStorageError::Io)?;
        f.sync_all()
            .await
            .map_err(PirateStorageError::Io)?;
    }
    if let Err(e) = tokio::fs::rename(&tmp, &final_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    if let Some(d) = db {
        let _ = maybe_reconcile_db(d, &cfg.root).await?;
    }
    Ok(n)
}

/// Commit a file already materialized in `<root>/.pirate-tmp/` by atomic rename to `rel_path`
/// (same rules as [`store_uploaded_file`], but no in-memory buffer). Caller must place `tmp_path`
/// under the reserved tmp directory.
pub async fn commit_uploaded_temp_file(
    cfg: &PirateStorageConfig,
    db: Option<&DbStore>,
    rel_path: &str,
    tmp_path: &Path,
) -> Result<u64, PirateStorageError> {
    ensure_configured(cfg)?;
    let tmp_dir = cfg.root.join(RESERVED_TMP);
    if !tmp_path.starts_with(&tmp_dir) {
        return Err(PirateStorageError::InvalidPath(
            "commit_uploaded_temp_file: temp path is not under PIRATE_STORAGE_ROOT/.pirate-tmp/".into(),
        ));
    }
    let meta = tokio::fs::metadata(tmp_path)
        .await
        .map_err(PirateStorageError::Io)?;
    if !meta.is_file() {
        return Err(PirateStorageError::InvalidPath(
            "commit_uploaded_temp_file: temp is not a file".into(),
        ));
    }
    let n = meta.len();
    if n > cfg.max_upload_bytes {
        return Err(PirateStorageError::InvalidPath(
            "file exceeds PIRATE_STORAGE_MAX_UPLOAD_BYTES".to_string(),
        ));
    }
    if normalize_rel_path(rel_path)?.is_empty() {
        return Err(PirateStorageError::InvalidPath(
            "path required for file upload".to_string(),
        ));
    }
    let final_path = resolve_path(&cfg.root, rel_path)?;
    if final_path.is_dir() {
        return Err(PirateStorageError::InvalidPath(
            "path is a directory".to_string(),
        ));
    }
    let old: u64 = if final_path.is_file() {
        final_path
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        0
    };
    if cfg.max_bytes > 0 {
        let used = walk_storage_used_bytes(&cfg.root);
        let new_total = used.saturating_sub(old).saturating_add(n);
        if new_total > cfg.max_bytes {
            let need = new_total - cfg.max_bytes;
            return Err(PirateStorageError::Quota {
                used: new_total,
                max: cfg.max_bytes,
                need,
            });
        }
    }
    if let Some(p) = final_path.parent() {
        tokio::fs::create_dir_all(p)
            .await
            .map_err(PirateStorageError::Io)?;
    }
    if let Err(e) = tokio::fs::rename(tmp_path, &final_path).await {
        return Err(e.into());
    }
    if let Some(d) = db {
        let _ = maybe_reconcile_db(d, &cfg.root).await?;
    }
    Ok(n)
}

// --- Archive extraction (zip / tar / tar.gz / tgz) ---

/// Whether `name` looks like a supported archive (by extension only).
pub fn is_supported_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

fn detect_archive_kind(name: &str) -> Option<ArchiveKind> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        return Some(ArchiveKind::Zip);
    }
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return Some(ArchiveKind::TarGz);
    }
    if lower.ends_with(".tar") {
        return Some(ArchiveKind::Tar);
    }
    None
}

/// Normalizes a path from inside an archive: no `..`, no `\`, no absolute junk.
fn safe_inner_path(raw: &str) -> Result<String, PirateStorageError> {
    let t = raw.trim().replace('\\', "/");
    if t.is_empty() || t == "." {
        return Err(PirateStorageError::InvalidPath("empty entry path in archive".into()));
    }
    let mut out: Vec<String> = Vec::new();
    for p in t.split('/') {
        if p.is_empty() || p == "." {
            continue;
        }
        if p == ".." {
            return Err(PirateStorageError::InvalidPath("path traversal in archive entry".into()));
        }
        if p == RESERVED_TMP {
            return Err(PirateStorageError::InvalidPath("reserved name in archive".into()));
        }
        if p.as_bytes().len() > MAX_NAME_BYTES {
            return Err(PirateStorageError::NameTooLong(p.to_string()));
        }
        out.push(p.to_string());
    }
    if out.is_empty() {
        return Err(PirateStorageError::InvalidPath("empty entry after normalize".into()));
    }
    if out.len() > MAX_PATH_COMPONENTS {
        return Err(PirateStorageError::InvalidPath("path too deep in archive".into()));
    }
    Ok(out.join("/"))
}

fn join_target_rel(target_base: &str, inner: &str) -> String {
    let t = target_base.trim();
    if t.is_empty() {
        inner.to_string()
    } else {
        format!("{}/{}", t, inner)
    }
}

/// Parent directory relative to storage root, or empty if archive is in root.
fn parent_of_rel(rel: &str) -> String {
    let t = rel.trim();
    if t.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = t.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 1 {
        return String::new();
    }
    parts[..parts.len() - 1].join("/")
}

fn precheck_quota_on_disk(root: &Path, max_bytes: u64) -> Result<(), PirateStorageError> {
    if max_bytes == 0 {
        return Ok(());
    }
    let used = walk_storage_used_bytes(root);
    if used > max_bytes {
        // Already over; still allow? Treat as no room.
        return Err(PirateStorageError::Quota {
            used,
            max: max_bytes,
            need: 0,
        });
    }
    Ok(())
}

fn path_exists(rel: &str, root: &Path) -> bool {
    resolve_path(root, rel)
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Apply conflict policy for an existing `dest` path when we are about to create `as_file` (true) or directory (false).
fn apply_conflict(
    _root: &Path,
    rel_dest: &str,
    dest: &Path,
    as_file: bool,
    mode: StorageExtractConflictMode,
) -> Result<(), PirateStorageError> {
    if !dest.exists() {
        return Ok(());
    }
    let rel_s = rel_dest.to_string();
    match mode {
        StorageExtractConflictMode::Abort => Err(PirateStorageError::ExtractConflict { path: rel_s }),
        StorageExtractConflictMode::Overwrite => {
            if as_file {
                if dest.is_file() {
                    return Ok(());
                }
                if dest.is_dir() {
                    // Cannot overwrite a directory with a file without delete.
                    Err(PirateStorageError::ExtractConflict { path: rel_s })
                } else {
                    Ok(())
                }
            } else {
                // Merging into directory: allow if it is a directory
                if dest.is_dir() {
                    Ok(())
                } else {
                    Err(PirateStorageError::ExtractConflict { path: rel_s })
                }
            }
        }
        StorageExtractConflictMode::DeleteAndOverwrite => {
            if dest.is_file() {
                std::fs::remove_file(dest).map_err(PirateStorageError::Io)?;
            } else if dest.is_dir() {
                std::fs::remove_dir_all(dest).map_err(PirateStorageError::Io)?;
            }
            Ok(())
        }
    }
}

/// Use [`zip::read::ZipFile::mangled_name`] (handles absolute paths, mixed separators, odd
/// central-directory names) then our `safe_inner_path` — more reliable than `name()` alone.
fn try_zip_entry_inner(
    entry: &zip::read::ZipFile<'_>,
) -> Result<String, PirateStorageError> {
    let m = entry.mangled_name();
    let raw = m.to_string_lossy();
    let t = raw.trim();
    if t.is_empty() {
        return Err(PirateStorageError::InvalidPath("empty entry path in archive".into()));
    }
    safe_inner_path(t.trim_end_matches('/').trim_end_matches('\\'))
}

fn is_zip_entry_dir(entry: &zip::read::ZipFile<'_>, name: &str) -> bool {
    if name.ends_with('/') || name.ends_with('\\') {
        return true;
    }
    if let Some(m) = entry.unix_mode() {
        if (m & 0o170_000) == 0o040_000 {
            return true;
        }
    }
    entry.is_dir()
}

fn extract_zip(
    root: &Path,
    archive_path: &Path,
    target_base: &str,
    mode: StorageExtractConflictMode,
    view: &mut StorageExtractView,
) -> Result<(), PirateStorageError> {
    let file = std::fs::File::open(archive_path).map_err(PirateStorageError::Io)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| PirateStorageError::CorruptArchive(e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| PirateStorageError::CorruptArchive(e.to_string()))?;
        let name = entry.name();
        if name.is_empty() {
            continue;
        }
        let inner = match try_zip_entry_inner(&entry) {
            Ok(s) => s,
            Err(e) => {
                view.warnings
                    .push(format!("skip bad entry {name:?}: {e}"));
                view.skipped = view.skipped.saturating_add(1);
                continue;
            }
        };
        let rel = join_target_rel(target_base, &inner);
        if let Err(e) = normalize_rel_path(&rel) {
            view
                .warnings
                .push(format!("skip {name:?} (invalid rel {rel}): {e}"));
            view.skipped = view.skipped.saturating_add(1);
            continue;
        }
        let dest = resolve_path(root, &rel)?;
        if is_zip_entry_dir(&entry, name) {
            if path_exists(&rel, root) {
                apply_conflict(
                    root,
                    &rel,
                    &dest,
                    false,
                    mode,
                )?;
            } else if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p).map_err(PirateStorageError::Io)?;
            }
            std::fs::create_dir_all(&dest).map_err(PirateStorageError::Io)?;
            view.created_dirs = view.created_dirs.saturating_add(1);
            continue;
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(PirateStorageError::Io)?;
        }
        apply_conflict(root, &rel, &dest, true, mode)?;
        let mut out = std::fs::File::create(&dest).map_err(PirateStorageError::Io)?;
        std::io::copy(&mut entry, &mut out).map_err(PirateStorageError::Io)?;
        view.extracted_files = view.extracted_files.saturating_add(1);
    }
    Ok(())
}

fn extract_tar_entries<R: std::io::Read>(
    root: &Path,
    read: R,
    target_base: &str,
    mode: StorageExtractConflictMode,
    view: &mut StorageExtractView,
) -> Result<(), PirateStorageError> {
    let mut archive = tar::Archive::new(read);
    for entry in archive.entries().map_err(|e| PirateStorageError::CorruptArchive(e.to_string()))? {
        let mut entry = entry.map_err(|e| PirateStorageError::CorruptArchive(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| PirateStorageError::CorruptArchive(e.to_string()))?
            .to_string_lossy()
            .to_string();
        if path.is_empty() {
            continue;
        }
        let inner = match safe_inner_path(path.trim_end_matches('/')) {
            Ok(s) => s,
            Err(e) => {
                view.warnings.push(format!("skip bad path {path:?}: {e}"));
                view.skipped = view.skipped.saturating_add(1);
                continue;
            }
        };
        let rel = join_target_rel(target_base, &inner);
        if let Err(e) = normalize_rel_path(&rel) {
            view
                .warnings
                .push(format!("skip (invalid rel {rel}): {e}"));
            view.skipped = view.skipped.saturating_add(1);
            continue;
        }
        let dest = resolve_path(root, &rel)?;
        let is_dir = entry.header().entry_type().is_dir();
        if is_dir {
            if path_exists(&rel, root) {
                apply_conflict(root, &rel, &dest, false, mode)?;
            } else if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p).map_err(PirateStorageError::Io)?;
            }
            std::fs::create_dir_all(&dest).map_err(PirateStorageError::Io)?;
            view.created_dirs = view.created_dirs.saturating_add(1);
            continue;
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(PirateStorageError::Io)?;
        }
        apply_conflict(root, &rel, &dest, true, mode)?;
        let mut out = std::fs::File::create(&dest).map_err(PirateStorageError::Io)?;
        std::io::copy(&mut entry, &mut out).map_err(PirateStorageError::Io)?;
        view.extracted_files = view.extracted_files.saturating_add(1);
    }
    Ok(())
}

/// Synchronous extract; call from `spawn_blocking`.
fn extract_archive_sync(
    cfg: &PirateStorageConfig,
    archive_rel: &str,
    target_dir_rel: Option<&str>,
    mode: StorageExtractConflictMode,
) -> Result<StorageExtractView, PirateStorageError> {
    ensure_configured(cfg)?;
    precheck_quota_on_disk(&cfg.root, cfg.max_bytes)?;
    let archive_path = resolve_existing_path(&cfg.root, archive_rel)?;
    if !archive_path.is_file() {
        return Err(PirateStorageError::InvalidPath("archive path must be a file".into()));
    }
    let name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let kind = detect_archive_kind(name).ok_or_else(|| {
        PirateStorageError::UnsupportedArchive(format!("{name} (expected .zip, .tar, .tar.gz, .tgz)"))
    })?;
    let target_base: String = match target_dir_rel {
        None => parent_of_rel(archive_rel),
        Some(s) if s.trim().is_empty() => parent_of_rel(archive_rel),
        Some(s) => normalize_rel_path(s)?,
    };
    let p = resolve_path(&cfg.root, &target_base)?;
    if !p.exists() {
        std::fs::create_dir_all(&p).map_err(PirateStorageError::Io)?;
    } else if !p.is_dir() {
        return Err(PirateStorageError::InvalidPath("target_dir must be a directory".into()));
    }
    let mut view = StorageExtractView {
        ok: true,
        extracted_files: 0,
        created_dirs: 0,
        skipped: 0,
        warnings: Vec::new(),
    };
    let root = cfg.root.clone();
    match kind {
        ArchiveKind::Zip => {
            extract_zip(
                &root,
                &archive_path,
                &target_base,
                mode,
                &mut view,
            )?;
        }
        ArchiveKind::Tar => {
            let file = std::fs::File::open(&archive_path).map_err(PirateStorageError::Io)?;
            extract_tar_entries(&root, file, &target_base, mode, &mut view)?;
        }
        ArchiveKind::TarGz => {
            let file = std::fs::File::open(&archive_path).map_err(PirateStorageError::Io)?;
            let gz = flate2::read::GzDecoder::new(file);
            extract_tar_entries(&root, gz, &target_base, mode, &mut view)?;
        }
    }
    // Post-check quota: if we went over, warn in reconcile path (DB) — walk_storage_used_bytes used by maybe_reconcile_db
    if cfg.max_bytes > 0 {
        let used = walk_storage_used_bytes(&cfg.root);
        if used > cfg.max_bytes {
            return Err(PirateStorageError::Quota {
                used,
                max: cfg.max_bytes,
                need: used - cfg.max_bytes,
            });
        }
    }
    Ok(view)
}

/// Extract an archive that already lives under `PIRATE_STORAGE_ROOT`.
pub async fn extract_archive(
    cfg: &PirateStorageConfig,
    db: Option<&DbStore>,
    archive_rel: &str,
    target_dir_rel: Option<&str>,
    mode: StorageExtractConflictMode,
) -> Result<StorageExtractView, PirateStorageError> {
    let root = cfg.root.clone();
    let cfg = cfg.clone();
    let ar = archive_rel.to_string();
    let td = target_dir_rel.map(|s| s.to_string());
    let v = tokio::task::spawn_blocking(move || {
        extract_archive_sync(
            &cfg,
            &ar,
            td.as_deref(),
            mode,
        )
    })
    .await
    .map_err(|e| PirateStorageError::InvalidPath(e.to_string()))??;
    if let Some(d) = db {
        let _ = maybe_reconcile_db(d, &root).await?;
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_double_dot() {
        assert!(normalize_rel_path("a/../b").is_err());
    }

    #[test]
    fn walk_skips_tmp() {
        let t = tempfile::tempdir().expect("tmp");
        let r = t.path();
        fs::create_dir(r.join(RESERVED_TMP)).unwrap();
        fs::write(r.join("a.txt"), b"ab").unwrap();
        assert_eq!(walk_storage_used_bytes(r), 2);
    }

    #[test]
    fn safe_inner_path_rejects_dotdot() {
        assert!(super::safe_inner_path("a/../b").is_err());
    }

    #[test]
    fn supported_archive_extensions() {
        assert!(super::is_supported_archive_name("x.zip"));
        assert!(super::is_supported_archive_name("A.TGZ"));
        assert!(!super::is_supported_archive_name("readme.txt"));
    }
}
