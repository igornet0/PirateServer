//! HTTP handlers for `/api/v1/data-sources` (SMB mounts + browse).

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use deploy_control::{DataSourceItemView, DataSourcesListView, SmbBrowseEntry, SmbBrowseView};
use serde::Deserialize;
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path as FsPath, PathBuf};
use uuid::Uuid;

use crate::error::ApiError;
use crate::ApiState;

#[cfg(unix)]
fn set_secret_file_permissions(path: &FsPath) -> std::io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(windows)]
fn set_secret_file_permissions(_path: &FsPath) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn set_secret_file_permissions(_path: &FsPath) -> std::io::Result<()> {
    Ok(())
}

#[derive(Deserialize)]
pub struct PostSmbBody {
    pub label: String,
    pub host: String,
    pub share: String,
    /// Subfolder inside the share (optional).
    pub folder: String,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct PostConnectionBody {
    pub kind: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub ssl: bool,
}

const CONNECTION_KINDS: &[&str] = &[
    "clickhouse", "oracle", "mysql", "mssql", "mongodb", "redis", "postgresql",
];

#[derive(Deserialize)]
pub struct BrowseQuery {
    /// Relative path under the share mount + configured subfolder.
    #[serde(default)]
    pub path: String,
}

fn validate_host(host: &str) -> Result<(), ApiError> {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return Err(ApiError::bad_request("invalid host"));
    }
    if !h
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request("invalid host"));
    }
    Ok(())
}

fn validate_share(share: &str) -> Result<(), ApiError> {
    let s = share.trim();
    if s.is_empty() || s.len() > 80 {
        return Err(ApiError::bad_request("invalid share"));
    }
    if s.contains('/') || s.contains('\\') {
        return Err(ApiError::bad_request("invalid share"));
    }
    Ok(())
}

fn normalize_folder(folder: &str) -> Result<String, ApiError> {
    let t = folder.trim().trim_matches('/').replace('\\', "/");
    if t.contains("..") {
        return Err(ApiError::bad_request("invalid folder"));
    }
    Ok(t)
}

fn validate_label(label: &str) -> Result<(), ApiError> {
    let t = label.trim();
    if t.is_empty() || t.len() > 128 {
        return Err(ApiError::bad_request("invalid label"));
    }
    Ok(())
}

pub async fn api_data_sources_list(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<DataSourcesListView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    s.plane.data_sources_list().await
        .map(Json)
        .map_err(Into::into)
}

pub async fn api_post_smb(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PostSmbBody>,
) -> Result<Json<DataSourceItemView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    if s.plane.db.is_none() {
        return Err(ApiError::service_unavailable(
            "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)",
        ));
    }

    validate_label(&body.label)?;
    validate_host(&body.host)?;
    validate_share(&body.share)?;
    let folder = normalize_folder(&body.folder)?;
    let user = body.username.trim();
    if user.is_empty() || user.len() > 256 {
        return Err(ApiError::bad_request("invalid username"));
    }
    if body.password.len() > 512 {
        return Err(ApiError::bad_request("invalid password"));
    }

    let id = Uuid::new_v4();
    let mount_point = s.data_mounts_root.join(id.to_string());
    let cred_path = s
        .data_mounts_root
        .join(".creds")
        .join(format!("{id}.cred"));
    let mount_str = mount_point
        .to_str()
        .ok_or_else(|| ApiError::internal("mount path"))?
        .to_string();
    let cred_str = cred_path
        .to_str()
        .ok_or_else(|| ApiError::internal("cred path"))?
        .to_string();

    let host = body.host.trim().to_string();
    let share = body.share.trim().to_string();
    let unc = format!("//{host}/{share}");

    let cred_content = format!(
        "username={}\npassword={}\n",
        user,
        body.password.replace('\n', "")
    );

    let mount_script = s.smb_mount_script.clone();
    let umount_script = s.smb_umount_script.clone();
    let label = body.label.trim().to_string();

    let mount_result = tokio::task::spawn_blocking({
        let mount_str = mount_str.clone();
        let cred_str = cred_str.clone();
        move || -> Result<(), ApiError> {
            let cred_path = PathBuf::from(&cred_str);
            let parent = cred_path
                .parent()
                .ok_or_else(|| ApiError::internal("cred parent"))?;
            std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
            std::fs::write(&cred_path, cred_content.as_bytes())
                .map_err(|e| ApiError::internal(e.to_string()))?;
            set_secret_file_permissions(&cred_path).map_err(|e| ApiError::internal(e.to_string()))?;

            let out = std::process::Command::new("sudo")
                .arg(mount_script.as_os_str())
                .arg(&mount_str)
                .arg(&unc)
                .arg(&cred_str)
                .output()
                .map_err(|e| ApiError::internal(e.to_string()))?;

            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let _ = std::fs::remove_file(&cred_path);
                return Err(ApiError::bad_gateway(format!(
                    "mount failed: {stderr}"
                )));
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    mount_result?;

    if let Err(e) = s
        .plane
        .data_sources_insert_smb(
            id,
            &label,
            &host,
            &share,
            &folder,
            &mount_str,
            &cred_str,
            "mounted",
            None,
        )
        .await
    {
        let _ = tokio::task::spawn_blocking({
            let mount_str = mount_str.clone();
            let cred_str = cred_str.clone();
            let umount_script = umount_script.clone();
            move || {
                let _ = std::process::Command::new("sudo")
                    .arg(umount_script)
                    .arg(&mount_str)
                    .output();
                let _ = std::fs::remove_file(cred_str);
            }
        })
        .await;
        return Err(e.into());
    }

    Ok(Json(DataSourceItemView {
        id: id.to_string(),
        kind: "smb".to_string(),
        label,
        mount_state: Some("mounted".to_string()),
        smb_host: Some(host),
        smb_share: Some(share),
        smb_subpath: Some(folder),
        mount_point: Some(mount_str),
        last_error: None,
        config_json: None,
        has_credentials: Some(true),
    }))
}

pub async fn api_post_connection(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PostConnectionBody>,
) -> Result<Json<DataSourceItemView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    if s.plane.db.is_none() {
        return Err(ApiError::service_unavailable(
            "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)",
        ));
    }

    validate_label(&body.label)?;
    let kind = body.kind.trim().to_lowercase();
    if !CONNECTION_KINDS.contains(&kind.as_str()) {
        return Err(ApiError::bad_request("invalid connection kind"));
    }
    validate_conn_host(&body.host)?;

    let id = Uuid::new_v4();
    let mut cfg = serde_json::Map::new();
    cfg.insert("host".to_string(), json!(body.host.trim()));
    cfg.insert("port".to_string(), json!(body.port));
    if let Some(ref db) = body.database {
        let t = db.trim();
        if !t.is_empty() {
            cfg.insert("database".to_string(), json!(t));
        }
    }
    if let Some(ref u) = body.username {
        let t = u.trim();
        if !t.is_empty() {
            cfg.insert("username".to_string(), json!(t));
        }
    }
    cfg.insert("ssl".to_string(), json!(body.ssl));
    let config_json = serde_json::Value::Object(cfg);

    let cred_path_opt: Option<String> = if let Some(ref pw) = body.password {
        if pw.is_empty() {
            None
        } else {
            let cred_path = s
                .data_mounts_root
                .join(".creds")
                .join(format!("{id}.conn"));
            let cred_str = cred_path
                .to_str()
                .ok_or_else(|| ApiError::internal("cred path"))?
                .to_string();
            tokio::task::spawn_blocking({
                let cred_path = cred_path.clone();
                let pw = pw.clone();
                move || -> Result<(), ApiError> {
                    let parent = cred_path
                        .parent()
                        .ok_or_else(|| ApiError::internal("cred parent"))?;
                    std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
                    std::fs::write(&cred_path, pw.as_bytes())
                        .map_err(|e| ApiError::internal(e.to_string()))?;
                    set_secret_file_permissions(&cred_path)
                        .map_err(|e| ApiError::internal(e.to_string()))?;
                    Ok(())
                }
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
            Some(cred_str)
        }
    } else {
        None
    };

    s.plane
        .data_sources_insert_connection(
            id,
            kind.as_str(),
            body.label.trim(),
            &config_json,
            cred_path_opt.as_deref(),
            "connected",
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let label = body.label.trim().to_string();
    Ok(Json(DataSourceItemView {
        id: id.to_string(),
        kind: kind.clone(),
        label,
        mount_state: Some("connected".to_string()),
        smb_host: None,
        smb_share: None,
        smb_subpath: None,
        mount_point: None,
        last_error: None,
        config_json: Some(strip_secrets_from_config(&config_json)),
        has_credentials: Some(cred_path_opt.is_some()),
    }))
}

fn strip_secrets_from_config(v: &serde_json::Value) -> serde_json::Value {
    let mut out = v.clone();
    if let Some(m) = out.as_object_mut() {
        m.remove("password");
        m.remove("passwd");
    }
    out
}

fn validate_conn_host(host: &str) -> Result<(), ApiError> {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return Err(ApiError::bad_request("invalid host"));
    }
    for ch in h.chars() {
        if ch.is_ascii_whitespace()
            || ch == '`'
            || ch == '$'
            || ch == ';'
            || ch == '|'
            || ch == '&'
        {
            return Err(ApiError::bad_request("invalid host"));
        }
    }
    Ok(())
}

/// True when `path` resolves under `root` (both canonicalized). Used to only remove dirs we created under data mounts root.
fn dir_is_under_root(path: &FsPath, root: &FsPath) -> bool {
    let Ok(canonical_path) = path.canonicalize() else {
        return false;
    };
    let Ok(canonical_root) = root.canonicalize() else {
        return false;
    };
    canonical_path.starts_with(canonical_root.as_path())
}

fn remove_smb_mount_artifacts(
    mount_point: &FsPath,
    data_mounts_root: &FsPath,
    credentials_path: Option<&str>,
) {
    if let Some(p) = credentials_path {
        let _ = std::fs::remove_file(p);
    }
    if dir_is_under_root(mount_point, data_mounts_root) {
        let _ = std::fs::remove_dir_all(mount_point);
    }
}

pub async fn api_data_sources_delete(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    if s.plane.db.is_none() {
        return Err(ApiError::service_unavailable(
            "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)",
        ));
    }

    let row = s
        .plane
        .data_sources_get_smb(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::bad_request("unknown data source"))?;

    if row.kind == "smb" {
        let mount_str = row
            .mount_point
            .clone()
            .ok_or_else(|| ApiError::internal("mount_point"))?;
        let umount_script = s.smb_umount_script.clone();
        let cred = row.credentials_path.clone();
        let mount_pb = PathBuf::from(&mount_str);
        let data_root = s.data_mounts_root.clone();

        tokio::task::spawn_blocking(move || {
            let _ = std::process::Command::new("sudo")
                .arg(umount_script)
                .arg(&mount_str)
                .output();
            remove_smb_mount_artifacts(&mount_pb, &data_root, cred.as_deref());
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    } else if let Some(ref cred) = row.credentials_path {
        let p = cred.clone();
        tokio::task::spawn_blocking(move || {
            let _ = std::fs::remove_file(p);
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    let deleted = s
        .plane
        .data_sources_delete_row(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !deleted {
        return Err(ApiError::bad_request("not found"));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn safe_join_under(base: &FsPath, rel: &str) -> Result<PathBuf, ApiError> {
    let rel = rel.trim().trim_start_matches('/');
    if rel.contains("..") {
        return Err(ApiError::bad_request("invalid path"));
    }
    for part in rel.split('/') {
        if part == ".." || part.contains('\\') {
            return Err(ApiError::bad_request("invalid path"));
        }
    }
    if rel.is_empty() {
        return Ok(base.to_path_buf());
    }
    Ok(base.join(rel))
}

pub async fn api_smb_browse(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<SmbBrowseView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    if s.plane.db.is_none() {
        return Err(ApiError::service_unavailable(
            "metadata database is not configured (set DEPLOY_SQLITE_URL or DATABASE_URL)",
        ));
    }

    let row = s
        .plane
        .data_sources_get_smb(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::bad_request("unknown data source"))?;

    if row.kind != "smb" {
        return Err(ApiError::bad_request("not an SMB source"));
    }
    if row.mount_state != "mounted" {
        return Err(ApiError::service_unavailable("SMB share is not mounted"));
    }

    let mount = PathBuf::from(
        row.mount_point
            .as_ref()
            .ok_or_else(|| ApiError::internal("mount_point"))?,
    );
    let sub = row
        .smb_subpath
        .as_deref()
        .unwrap_or("")
        .trim()
        .trim_start_matches('/');
    let base = if sub.is_empty() {
        mount.clone()
    } else {
        mount.join(sub)
    };

    let target = safe_join_under(&base, &q.path)?;
    if !target.starts_with(&mount) {
        return Err(ApiError::bad_request("path escapes mount"));
    }

    let entries = tokio::task::spawn_blocking(move || -> Result<Vec<SmbBrowseEntry>, ApiError> {
        let read = std::fs::read_dir(&target)
            .map_err(|e| ApiError::bad_gateway(format!("read_dir: {e}")))?;
        let mut out = Vec::new();
        for ent in read {
            let ent = ent.map_err(|e| ApiError::internal(e.to_string()))?;
            let meta = ent.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = if is_dir {
                None
            } else {
                meta.as_ref().map(|m| m.len())
            };
            out.push(SmbBrowseEntry {
                name: ent.file_name().to_string_lossy().to_string(),
                is_dir,
                size,
            });
        }
        out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        Ok(out)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;

    let display_path = q.path.trim().to_string();
    Ok(Json(SmbBrowseView {
        source_id: id.to_string(),
        path: display_path,
        entries,
    }))
}
