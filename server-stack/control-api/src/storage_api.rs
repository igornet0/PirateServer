//! Handlers for `GET/POST/DELETE /api/v1/storage/*` (Pirate file manager).

use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::multipart::Field;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use deploy_control::{
    commit_uploaded_temp_file, create_folder, extract_archive, list_tree, normalize_rel_path,
    parse_bind_source_prefixes, remove_dir, remove_file, rename_path, resolve_existing_path,
    storage_bind_sources_view, storage_usage, storage_usage_no_db, PirateStorageConfig,
    StorageBindSourcesView, StorageExtractConflictMode, StorageExtractView, StorageListView,
    StorageUsageView,
};
use futures::stream::StreamExt;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::ApiError;
use crate::ApiState;

const RESERVED_TMP: &str = ".pirate-tmp";

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

fn session_is_expired_ms(updated_at_ms: i64, ttl_secs: u64, now_ms: i64) -> bool {
    let ttl_ms = (ttl_secs as i64).saturating_mul(1000);
    now_ms.saturating_sub(updated_at_ms) > ttl_ms
}

pub(crate) async fn cleanup_expired_storage_upload_sessions(s: &ApiState) {
    let now = now_ms();
    let mut expired_paths = Vec::<PathBuf>::new();
    {
        let mut sessions = s.storage_upload_sessions.lock().await;
        sessions.retain(|_, sess| {
            if session_is_expired_ms(
                sess.updated_at_ms,
                s.storage_upload_session_ttl_secs,
                now,
            ) {
                expired_paths.push(sess.tmp_path.clone());
                false
            } else {
                true
            }
        });
    }
    for p in expired_paths {
        let _ = tokio::fs::remove_file(&p).await;
    }
}

fn storage_cfg(s: &ApiState) -> Result<PirateStorageConfig, ApiError> {
    let root = s
        .storage_root
        .as_ref()
        .ok_or_else(|| {
            ApiError::service_unavailable("Pirate storage is not configured (PIRATE_STORAGE_ROOT)")
        })?;
    if !root.is_dir() {
        return Err(ApiError::service_unavailable(
            "Pirate storage root is not a directory; create PIRATE_STORAGE_ROOT on the host",
        ));
    }
    Ok(PirateStorageConfig {
        root: root.clone(),
        max_bytes: s.storage_max_bytes,
        max_upload_bytes: s.storage_max_upload_bytes,
    })
}

async fn stream_multipart_file_field_to_path(
    mut field: Field<'_>,
    out_path: &std::path::Path,
    max: u64,
) -> Result<u64, ApiError> {
    let mut f = tokio::fs::File::create(out_path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut total: u64 = 0;
    while let Some(item) = field.next().await {
        let chunk = item.map_err(|e| ApiError::bad_request(format!("multipart read: {e}")))?;
        let c = chunk.len() as u64;
        if total.saturating_add(c) > max {
            drop(f);
            let _ = tokio::fs::remove_file(out_path).await;
            return Err(ApiError::bad_request(
                "file exceeds PIRATE_STORAGE_MAX_UPLOAD_BYTES (or effective DEPLOY_MAX_UPLOAD_BYTES)",
            ));
        }
        total = total.saturating_add(c);
        f.write_all(&chunk)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    f.sync_all()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if total == 0 {
        let _ = tokio::fs::remove_file(out_path).await;
        return Err(ApiError::bad_request("empty file field"));
    }
    Ok(total)
}

/// Resumable upload: temp file and metadata in memory.
#[derive(Clone, Debug)]
pub(crate) struct StorageUploadSessionState {
    pub rel_path: String,
    pub file_bytes: u64,
    pub file_sha256: String,
    pub tmp_path: PathBuf,
    pub received_bytes: u64,
    pub updated_at_ms: i64,
}

#[derive(Deserialize)]
pub struct StorageUploadSessionCreateBody {
    pub path: String,
    pub file_bytes: u64,
    pub file_sha256: String,
}

#[derive(serde::Serialize)]
pub struct StorageUploadSessionCreateOut {
    pub upload_id: String,
    pub chunk_bytes: usize,
    pub received_bytes: u64,
}

#[derive(serde::Serialize)]
pub struct StorageUploadSessionChunkOut {
    pub received_bytes: u64,
}

/// `POST /api/v1/storage/upload-sessions` — JSON: path, file_bytes, file_sha256
pub async fn api_storage_upload_session_create(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<StorageUploadSessionCreateBody>,
) -> Result<Json<StorageUploadSessionCreateOut>, ApiError> {
    cleanup_expired_storage_upload_sessions(&s).await;
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    if body.file_bytes == 0 || body.file_bytes > cfg.max_upload_bytes {
        return Err(ApiError::bad_request(format!(
            "file_bytes must be between 1 and {} (per-file cap)",
            cfg.max_upload_bytes
        )));
    }
    if !is_sha256_hex(body.file_sha256.as_str()) {
        return Err(ApiError::bad_request(
            "file_sha256 must be a 64-char hex string",
        ));
    }
    if normalize_rel_path(&body.path)?.is_empty() {
        return Err(ApiError::bad_request("path required for upload session"));
    }
    let upload_id = Uuid::new_v4().to_string();
    let tmp_dir = cfg.root.join(RESERVED_TMP);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let tmp_path = tmp_dir.join(format!("upload-{upload_id}.part"));
    tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| ApiError::internal(format!("create session temp file: {e}")))?;
    let now = now_ms();
    let session = StorageUploadSessionState {
        rel_path: body.path.trim().to_string(),
        file_bytes: body.file_bytes,
        file_sha256: body.file_sha256.to_lowercase(),
        tmp_path,
        received_bytes: 0,
        updated_at_ms: now,
    };
    {
        let mut sessions = s.storage_upload_sessions.lock().await;
        sessions.insert(upload_id.clone(), session);
    }
    Ok(Json(StorageUploadSessionCreateOut {
        upload_id,
        chunk_bytes: s.storage_upload_session_chunk_bytes,
        received_bytes: 0,
    }))
}

/// `PUT /api/v1/storage/upload-sessions/:upload_id/chunk?offset=`
pub async fn api_storage_upload_session_chunk(
    State(s): State<ApiState>,
    Path(upload_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<StorageUploadSessionChunkOut>, ApiError> {
    cleanup_expired_storage_upload_sessions(&s).await;
    crate::check_api_bearer(&s, &headers)?;
    let offset: u64 = query
        .get("offset")
        .ok_or_else(|| ApiError::bad_request("missing query parameter `offset`"))?
        .parse()
        .map_err(|_| ApiError::bad_request("invalid query parameter `offset`"))?;
    let snapshot = {
        let mut sessions = s.storage_upload_sessions.lock().await;
        let Some(sess) = sessions.get(&upload_id).cloned() else {
            return Err(ApiError::bad_request("unknown storage upload session"));
        };
        let now = now_ms();
        if session_is_expired_ms(
            sess.updated_at_ms,
            s.storage_upload_session_ttl_secs,
            now,
        ) {
            let removed = sessions.remove(&upload_id);
            drop(sessions);
            if let Some(r) = removed {
                let _ = tokio::fs::remove_file(&r.tmp_path).await;
            }
            return Err(ApiError::bad_request("storage upload session expired"));
        }
        sess
    };
    if offset != snapshot.received_bytes {
        return Err(ApiError::bad_request(format!(
            "invalid offset: expected current={} got offset={}",
            snapshot.received_bytes, offset
        )));
    }
    if body.is_empty() {
        return Err(ApiError::bad_request("chunk body must not be empty"));
    }
    let next = offset.saturating_add(body.len() as u64);
    if next > snapshot.file_bytes {
        return Err(ApiError::bad_request(format!(
            "chunk exceeds declared file_bytes (declared={}, attempted_end={})",
            snapshot.file_bytes, next
        )));
    }
    if let Some(h) = headers.get("x-chunk-sha256") {
        let expected = h
            .to_str()
            .map_err(|_| ApiError::bad_request("invalid x-chunk-sha256 header"))?;
        if !is_sha256_hex(expected) {
            return Err(ApiError::bad_request(
                "x-chunk-sha256 must be a 64-char hex string",
            ));
        }
        let actual = format!("{:x}", Sha256::digest(&body));
        if actual != expected.to_lowercase() {
            return Err(ApiError::bad_request("x-chunk-sha256 mismatch"));
        }
    }
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&snapshot.tmp_path)
        .await
        .map_err(|e| ApiError::internal(format!("storage session append open: {e}")))?;
    f.write_all(&body)
        .await
        .map_err(|e| ApiError::internal(format!("storage session append write: {e}")))?;
    f.sync_all()
        .await
        .map_err(|e| ApiError::internal(format!("storage session append fsync: {e}")))?;
    let now = now_ms();
    let updated = {
        let mut sessions = s.storage_upload_sessions.lock().await;
        let Some(sess) = sessions.get_mut(&upload_id) else {
            return Err(ApiError::bad_request("unknown storage upload session"));
        };
        if sess.received_bytes != offset {
            return Err(ApiError::bad_request(format!(
                "invalid offset: expected current={} got offset={}",
                sess.received_bytes, offset
            )));
        }
        sess.received_bytes = next;
        sess.updated_at_ms = now;
        sess.received_bytes
    };
    Ok(Json(StorageUploadSessionChunkOut {
        received_bytes: updated,
    }))
}

/// `POST /api/v1/storage/upload-sessions/:upload_id/complete`
pub async fn api_storage_upload_session_complete(
    State(s): State<ApiState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    cleanup_expired_storage_upload_sessions(&s).await;
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    let sess = {
        let mut sessions = s.storage_upload_sessions.lock().await;
        sessions
            .remove(&upload_id)
            .ok_or_else(|| ApiError::bad_request("unknown storage upload session"))?
    };
    if session_is_expired_ms(
        sess.updated_at_ms,
        s.storage_upload_session_ttl_secs,
        now_ms(),
    ) {
        let _ = tokio::fs::remove_file(&sess.tmp_path).await;
        return Err(ApiError::bad_request("storage upload session expired"));
    }
    if sess.received_bytes != sess.file_bytes {
        let _ = tokio::fs::remove_file(&sess.tmp_path).await;
        return Err(ApiError::bad_request(format!(
            "upload incomplete: received={} expected={}",
            sess.received_bytes, sess.file_bytes
        )));
    }
    let mut f = tokio::fs::File::open(&sess.tmp_path)
        .await
        .map_err(|e| ApiError::internal(format!("storage session open for sha256: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| ApiError::internal(format!("storage session read for sha256: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_sha256 = format!("{:x}", hasher.finalize());
    if actual_sha256 != sess.file_sha256 {
        let _ = tokio::fs::remove_file(&sess.tmp_path).await;
        return Err(ApiError::bad_request("file sha256 mismatch at complete step"));
    }
    let n = match commit_uploaded_temp_file(&cfg, db, &sess.rel_path, &sess.tmp_path).await {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&sess.tmp_path).await;
            return Err(e.into());
        }
    };
    let rel = sess.rel_path;
    Ok(Json(serde_json::json!({ "ok": true, "bytes": n, "path": rel })))
}

/// `DELETE /api/v1/storage/upload-sessions/:upload_id`
pub async fn api_storage_upload_session_delete(
    State(s): State<ApiState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    cleanup_expired_storage_upload_sessions(&s).await;
    crate::check_api_bearer(&s, &headers)?;
    let removed = {
        let mut sessions = s.storage_upload_sessions.lock().await;
        sessions.remove(&upload_id)
    };
    if let Some(sess) = removed {
        let _ = tokio::fs::remove_file(&sess.tmp_path).await;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct TreeQuery {
    #[serde(default)]
    pub path: String,
}

/// `GET /api/v1/storage/tree?path=`
pub async fn api_storage_tree(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<TreeQuery>,
) -> Result<Json<StorageListView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let v = list_tree(&cfg, &q.path).await?;
    Ok(Json(v))
}

/// `GET /api/v1/storage/usage`
pub async fn api_storage_usage(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<StorageUsageView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let Some(ref root) = s.storage_root else {
        return Err(ApiError::service_unavailable(
            "Pirate storage is not configured (PIRATE_STORAGE_ROOT)",
        ));
    };
    if !root.is_dir() {
        return Err(ApiError::service_unavailable("Pirate storage root is not available"));
    }
    let v = if let Some(ref db) = s.plane.db {
        storage_usage(db, root, s.storage_max_bytes).await?
    } else {
        storage_usage_no_db(root, s.storage_max_bytes)?
    };
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct CreateFolderBody {
    pub path: String,
}

/// `POST /api/v1/storage/folders` JSON `{"path":"a/b"}`
pub async fn api_storage_folders_create(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateFolderBody>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    create_folder(&cfg, &body.path).await?;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
pub struct RenameBody {
    pub from: String,
    pub to: String,
}

/// `PATCH /api/v1/storage/folders` — rename or move a directory.
pub async fn api_storage_folders_patch(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    rename_path(&cfg, db, &body.from, &body.to).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/storage/folders?path=…&recursive=`
#[derive(Deserialize)]
pub struct DelFolderQuery {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

pub async fn api_storage_folders_delete(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<DelFolderQuery>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    remove_dir(&cfg, db, &q.path, q.recursive).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PATCH /api/v1/storage/files` — rename or move a file.
pub async fn api_storage_files_patch(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    rename_path(&cfg, db, &body.from, &body.to).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/storage/files` — multipart, fields: `path` (relative), `file` (binary, streamed to disk).
pub async fn api_storage_files_upload(
    State(s): State<ApiState>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let max = cfg.max_upload_bytes;
    let tmp_dir = cfg.root.join(RESERVED_TMP);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut rel: Option<String> = None;
    let mut file_tmp: Option<PathBuf> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        let name = field.name().map(|n| n.to_string()).unwrap_or_default();
        if name == "path" {
            let t = field
                .text()
                .await
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            rel = Some(t);
        } else if name == "file" {
            if file_tmp.is_some() {
                return Err(ApiError::bad_request("duplicate multipart field: file"));
            }
            let tmp = tmp_dir.join(Uuid::new_v4().to_string());
            stream_multipart_file_field_to_path(field, &tmp, max).await?;
            file_tmp = Some(tmp);
        }
    }
    let rel = rel.ok_or_else(|| ApiError::bad_request("missing multipart field: path"))?;
    let tmp = file_tmp.ok_or_else(|| ApiError::bad_request("missing multipart field: file"))?;
    let db = s.plane.db.as_deref();
    let n = match commit_uploaded_temp_file(&cfg, db, &rel, &tmp).await {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e.into());
        }
    };
    Ok(Json(serde_json::json!({ "ok": true, "bytes": n, "path": rel })))
}

#[derive(Deserialize)]
pub struct FilePathQuery {
    pub path: String,
}

/// `GET /api/v1/storage/files/download?path=`
pub async fn api_storage_files_download(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<FilePathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let p = resolve_existing_path(&cfg.root, &q.path)?;
    if !p.is_file() {
        if p.is_dir() {
            return Err(ApiError::bad_request("path is a directory, not a file"));
        }
        return Err(ApiError::not_found("file"));
    }
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let data = std::fs::read(&p).map_err(|e| ApiError::internal(e.to_string()))?;
    let dispo = format!("attachment; filename=\"{}\"", name.replace('\"', "'"));
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, dispo),
        ],
        axum::body::Body::from(data),
    ))
}

/// `DELETE /api/v1/storage/files?path=`
pub async fn api_storage_files_delete(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<FilePathQuery>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    remove_file(&cfg, db, &q.path).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct DeleteStorageFileBody {
    pub path: String,
}

/// `POST /api/v1/storage/delete-file` — JSON `{"path":"a/b"}`; same as `DELETE …/files?path=`.
/// Use this from clients behind reverse proxies that block `DELETE` / `PATCH`.
pub async fn api_storage_delete_file_post(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<DeleteStorageFileBody>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    remove_file(&cfg, db, &body.path).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct DeleteStorageFolderBody {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

/// `POST /api/v1/storage/delete-folder` — JSON `{"path":"a/b","recursive":false}`; same as `DELETE …/folders?…`.
pub async fn api_storage_delete_folder_post(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<DeleteStorageFolderBody>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    remove_dir(&cfg, db, &body.path, body.recursive).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/storage/rename` — JSON `{"from":"a","to":"b"}`; same as `PATCH …/files` or `…/folders`.
pub async fn api_storage_rename_post(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<RenameBody>,
) -> Result<StatusCode, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    rename_path(&cfg, db, &body.from, &body.to).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/storage/extract` — unpack zip/tar/tar.gz/tgz into storage.
#[derive(Deserialize)]
pub struct StorageExtractBody {
    pub archive_path: String,
    #[serde(default)]
    pub target_dir: Option<String>,
    /// `abort` | `overwrite` | `delete_and_overwrite`
    pub conflict_mode: StorageExtractConflictMode,
}

/// `POST /api/v1/storage/extract` JSON `StorageExtractBody`.
pub async fn api_storage_extract(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<StorageExtractBody>,
) -> Result<Json<StorageExtractView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let cfg = storage_cfg(&s)?;
    let db = s.plane.db.as_deref();
    let v = extract_archive(
        &cfg,
        db,
        body.archive_path.trim(),
        body.target_dir
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty()),
        body.conflict_mode,
    )
    .await?;
    Ok(Json(v))
}

/// `GET /api/v1/storage/bind-sources` — mount candidates from `/proc/mounts` (Linux) + active binds registry.
pub async fn api_storage_bind_sources(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<StorageBindSourcesView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let Some(ref root) = s.storage_root else {
        return Err(ApiError::service_unavailable(
            "Pirate storage is not configured (PIRATE_STORAGE_ROOT)",
        ));
    };
    let prefixes = parse_bind_source_prefixes(s.storage_bind_source_prefixes.as_deref());
    let view = storage_bind_sources_view(root, &prefixes, &s.storage_bind_state_path)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct StorageBindBody {
    pub source_path: String,
    pub volume_name: String,
}

/// `POST /api/v1/storage/bind` — JSON `source_path` (absolute dir), `volume_name` (under `volumes/`).
pub async fn api_storage_bind(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<StorageBindBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let _ = storage_cfg(&s)?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = body;
        return Err(ApiError::not_implemented(
            "storage bind is only supported on Linux hosts",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use deploy_control::{storage_bind_via_sudo, storage_bind_volume_name_ok};
        let src = body.source_path.trim().to_string();
        let vol = body.volume_name.trim().to_string();
        if src.len() > 4096 {
            return Err(ApiError::bad_request("source_path too long"));
        }
        if !std::path::Path::new(&src).is_absolute() {
            return Err(ApiError::bad_request("source_path must be absolute"));
        }
        if !storage_bind_volume_name_ok(&vol) {
            return Err(ApiError::bad_request(
                "invalid volume_name (letters, digits, ._- ; max 63 chars)",
            ));
        }
        let script = s.storage_bind_script.clone();
        let msg = tokio::task::spawn_blocking(move || storage_bind_via_sudo(&script, &src, &vol))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
        Ok(Json(serde_json::json!({ "ok": true, "message": msg })))
    }
}

#[derive(Deserialize)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct StorageUnbindBody {
    pub volume_name: String,
}

/// `POST /api/v1/storage/unbind` — JSON `volume_name` (same as used with bind).
pub async fn api_storage_unbind(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<StorageUnbindBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    let _ = storage_cfg(&s)?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = body;
        return Err(ApiError::not_implemented(
            "storage unbind is only supported on Linux hosts",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use deploy_control::{storage_bind_volume_name_ok, storage_unbind_via_sudo};
        let vol = body.volume_name.trim().to_string();
        if !storage_bind_volume_name_ok(&vol) {
            return Err(ApiError::bad_request(
                "invalid volume_name (letters, digits, ._- ; max 63 chars)",
            ));
        }
        let script = s.storage_bind_script.clone();
        let msg = tokio::task::spawn_blocking(move || storage_unbind_via_sudo(&script, &vol))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
        Ok(Json(serde_json::json!({ "ok": true, "message": msg })))
    }
}
