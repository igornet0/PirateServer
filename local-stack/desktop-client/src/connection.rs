//! Connection bundle parsing, pairing, and signed `GetStatus`.
//!
//! gRPC deploy URL, pairing, project id, and Ed25519 identity use the same files as the `pirate` CLI
//! (`deploy_client::config`: `connection.json` + `identity.json`). SQLite `connection` row mirrors
//! those fields for legacy UI queries; app-only columns (control-api JWT, etc.) stay in SQLite.

use deploy_auth::{
    attach_auth_metadata, load_or_create_identity, now_unix_ms, pair_request_canonical,
    pubkey_b64_url, verify_pair_response, ConnectionBundle,
};
use deploy_client::config::{
    self as client_config, load_connection, save_connection, StoredConnection,
};
use deploy_proto::deploy::{PairRequest, StatusRequest, StatusResponse};
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use rand_core::{OsRng, RngCore};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Once};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use parking_lot::Mutex;
use tonic::transport::Endpoint;
use tonic::Request;

use crate::bookmarks::bookmark_pairing_pubkey_for_url;
use crate::desktop_store;

static UNIFIED_CONFIG_MIGRATE: Once = Once::new();

const CONTROL_API_BASE_MODE_AUTO: &str = "auto";
const CONTROL_API_BASE_MODE_MANUAL: &str = "manual";
const GRPC_STATUS_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const GRPC_STATUS_RPC_TIMEOUT: Duration = Duration::from_secs(6);
const GRPC_STATUS_TOTAL_TIMEOUT: Duration = Duration::from_secs(7);
const GRPC_VERIFY_CACHE_TTL: Duration = Duration::from_secs(8);

static GRPC_VERIFY_CACHE: LazyLock<Mutex<HashMap<String, (GrpcConnectResult, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Attach signed gRPC metadata when paired (shared by host_stats, ssl_ops).
pub fn grpc_attach_if_paired<T>(
    req: &mut Request<T>,
    endpoint: &str,
    method: &str,
    project_id: &str,
) -> Result<(), String> {
    match load_signing_key_for_endpoint(endpoint) {
        Ok(None) => Ok(()),
        Ok(Some(sk)) => {
            attach_auth_metadata(req, &sk, method, project_id, "").map_err(|e| e.to_string())
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcConnectResult {
    pub endpoint: String,
    /// `DEPLOY_GRPC_PUBLIC_URL` from deploy-server (`StatusResponse.client_connect_url`).
    #[serde(default)]
    pub server_advertised_grpc_url: String,
    pub current_version: String,
    /// `[project].version` from deployed `pirate.toml` (GetStatus).
    pub project_version: String,
    pub state: String,
    /// HTTP base for control-api from server (`DEPLOY_CONTROL_API_PUBLIC_URL` / nginx).
    #[serde(default)]
    pub control_api_http_url: String,
    /// Direct control-api base from server (`DEPLOY_CONTROL_API_DIRECT_URL` or derived :8080).
    #[serde(default)]
    pub control_api_http_url_direct: String,
    /// deploy-server max artifact upload (bytes) when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_upload_bytes: Option<u64>,
}

fn default_project_id() -> String {
    "default".to_string()
}

fn signing_identity_path() -> Result<PathBuf, String> {
    client_config::identity_path()
        .ok_or_else(|| "no config directory for pirate-client".to_string())
}

fn legacy_desktop_identity_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("identity.json")
}

/// Run once at app startup: migrate legacy `PirateClient/identity.json` + SQLite pairing into
/// `deploy_client::config` so the `pirate` CLI sees the same connection.
pub fn ensure_unified_client_config_migrated() {
    UNIFIED_CONFIG_MIGRATE.call_once(|| {
        if let Err(e) = migrate_unified_client_config_inner() {
            tracing::warn!(%e, "unified client config migration skipped");
        }
    });
}

fn canon_connection_covers_signed_grpc(c: &StoredConnection) -> bool {
    c.paired && !c.url.trim().is_empty() && !c.server_pubkey_b64.trim().is_empty()
}

fn mirror_connection_row_to_sqlite(c: &StoredConnection) -> Result<(), String> {
    let db = desktop_store::open().map_err(|e| e.to_string())?;
    let paired = i64::from(c.paired);
    let pk = c.server_pubkey_b64.trim();
    let pk_param: Option<String> = if pk.is_empty() {
        None
    } else {
        Some(pk.to_string())
    };
    let pid = deploy_core::normalize_project_id(c.project_id.trim());
    db.execute(
        "UPDATE connection SET url = ?1, server_pubkey_b64 = ?2, paired = ?3, project_id = ?4 WHERE id = 1",
        params![c.url.clone(), pk_param, paired, pid],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn save_canon_connection(c: &StoredConnection) -> Result<(), String> {
    save_connection(c).map_err(|e| e.to_string())?;
    mirror_connection_row_to_sqlite(c)
}

fn load_legacy_sqlite_grpc_row() -> Option<(String, Option<String>, bool, String)> {
    let path = desktop_store::db_path();
    if !path.exists() {
        return None;
    }
    let db = rusqlite::Connection::open(&path).ok()?;
    let mut stmt = db
        .prepare("SELECT url, server_pubkey_b64, paired, project_id FROM connection WHERE id = 1")
        .ok()?;
    stmt.query_row([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)? != 0,
            row.get::<_, String>(3)?,
        ))
    })
    .ok()
}

fn migrate_legacy_identity_file_if_needed() -> Result<(), String> {
    let new_path = signing_identity_path()?;
    if new_path.exists() {
        return Ok(());
    }
    let old = legacy_desktop_identity_path();
    if !old.exists() {
        return Ok(());
    }
    if let Some(parent) = new_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::copy(&old, &new_path).map_err(|e| e.to_string())?;
    Ok(())
}

fn migrate_unified_client_config_inner() -> Result<(), String> {
    let canon = load_connection();
    if let Some(ref c) = canon {
        if canon_connection_covers_signed_grpc(c) {
            mirror_connection_row_to_sqlite(c)?;
            return Ok(());
        }
    }

    let Some((url, pk_opt, paired, project_id)) = load_legacy_sqlite_grpc_row() else {
        return Ok(());
    };
    if !(paired && !url.trim().is_empty()) {
        return Ok(());
    }
    let Some(pk) = pk_opt.filter(|s| !s.trim().is_empty()) else {
        return Ok(());
    };

    let should_write_json = match &canon {
        None => true,
        Some(c) => !canon_connection_covers_signed_grpc(c),
    };
    if !should_write_json {
        return Ok(());
    }

    migrate_legacy_identity_file_if_needed()?;

    let pid = project_id.trim();
    let body = StoredConnection {
        url: normalize_endpoint(&url),
        server_pubkey_b64: pk,
        paired: true,
        connection_kind: 0,
        project_id: if pid.is_empty() {
            default_project_id()
        } else {
            deploy_core::normalize_project_id(pid)
        },
    };
    save_connection(&body).map_err(|e| e.to_string())?;
    mirror_connection_row_to_sqlite(&body)?;
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn normalize_endpoint(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    let e = endpoint.trim();
    if e.is_empty() {
        return Err("endpoint is empty".into());
    }
    if !e.starts_with("http://") && !e.starts_with("https://") {
        return Err("endpoint must start with http:// or https://".into());
    }
    Ok(())
}

/// `true` when JSON is an object that includes `token` (full install bundle → Pair flow).
fn install_json_has_token(text: &str) -> Result<bool, String> {
    let t = text.trim();
    if !t.starts_with('{') {
        return Ok(false);
    }
    let v: Value = serde_json::from_str(t).map_err(|e| format!("invalid json: {e}"))?;
    let Some(obj) = v.as_object() else {
        return Err("expected a JSON object".into());
    };
    Ok(obj.contains_key("token"))
}

/// Extract URL from install JSON or legacy `export GRPC_ENDPOINT=…` / single URL line.
pub fn parse_grpc_endpoint_from_bundle(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("paste is empty".into());
    }

    if t.starts_with('{') {
        if install_json_has_token(t)? {
            let b = ConnectionBundle::parse(t).map_err(|e| e.to_string())?;
            return Ok(normalize_endpoint(&b.url));
        }
        let v: Value = serde_json::from_str(t).map_err(|e| format!("invalid json: {e}"))?;
        let Some(obj) = v.as_object() else {
            return Err("expected a JSON object".into());
        };
        let url = obj
            .get("url")
            .or_else(|| obj.get("endpoint"))
            .and_then(|x| x.as_str())
            .ok_or_else(|| {
                r#"expected "url" or "endpoint", or install JSON with "token" for pairing"#
                    .to_string()
            })?;
        validate_endpoint(url)?;
        return Ok(normalize_endpoint(url));
    }

    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() == 1 {
        let line = lines[0];
        if line.starts_with("http://") || line.starts_with("https://") {
            return Ok(normalize_endpoint(line));
        }
    }

    for line in t.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let rest = line.strip_prefix("export ").unwrap_or(line).trim();
        if let Some(val) = rest.strip_prefix("GRPC_ENDPOINT=") {
            let v = val.trim().trim_matches('"').trim_matches('\'').trim();
            if v.starts_with("http://") || v.starts_with("https://") {
                return Ok(normalize_endpoint(v));
            }
        }
    }

    Err(
        "expected {\"url\":\"http://…\"} or install JSON with token/url/pairing, or export GRPC_ENDPOINT=…, or a single http(s) URL"
            .into(),
    )
}

fn apply_control_api_hint_from_status(r: &StatusResponse) {
    // Always keep direct URL in sync (used as fallback for bootstrap, e.g. `ensure_nginx` when HTTPS is not up yet).
    let direct = r.control_api_http_url_direct.trim();
    if !direct.is_empty() && validate_endpoint(direct).is_ok() {
        let n = normalize_endpoint(direct);
        let _ = save_control_api_direct_url_hint(&n);
    } else {
        let _ = save_control_api_direct_url_hint("");
    }

    let chosen_raw = r.control_api_http_url.trim();
    let chosen_raw = if !chosen_raw.is_empty() {
        chosen_raw
    } else {
        r.control_api_http_url_direct.trim()
    };
    if chosen_raw.is_empty() {
        return;
    }
    let chosen = normalize_endpoint(chosen_raw);
    let current = load_control_api_base();
    if current.as_ref().map(|s| normalize_endpoint(s)) == Some(chosen.clone()) {
        return;
    }
    if current.is_some()
        && load_control_api_base_mode().as_deref() == Some(CONTROL_API_BASE_MODE_MANUAL)
    {
        return;
    }
    let _ = set_control_api_base_with_mode(&chosen, CONTROL_API_BASE_MODE_AUTO);
}

fn save_control_api_direct_url_hint(direct: &str) -> Result<(), String> {
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE connection SET control_api_direct_url = ?1 WHERE id = 1",
        params![direct.trim().to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Last known `GetStatus` `control_api_http_url_direct` (e.g. `http://host:8080`); for fallback when the public base is unreachable.
pub fn load_control_api_direct_url() -> Option<String> {
    let c = desktop_store::open().ok()?;
    let s: String = c
        .query_row(
            "SELECT control_api_direct_url FROM connection WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(normalize_endpoint(t))
    }
}

fn grpc_connect_result(endpoint: String, r: StatusResponse) -> GrpcConnectResult {
    apply_control_api_hint_from_status(&r);
    let server_advertised_grpc_url = normalize_endpoint(r.client_connect_url.as_str());
    GrpcConnectResult {
        endpoint,
        server_advertised_grpc_url,
        current_version: r.current_version,
        project_version: r.project_version,
        state: r.state,
        control_api_http_url: r.control_api_http_url,
        control_api_http_url_direct: r.control_api_http_url_direct,
        max_upload_bytes: (r.max_upload_bytes > 0).then_some(r.max_upload_bytes),
    }
}

/// When the saved connection is paired, returns the client signing key for gRPC auth.
pub fn load_signing_key_for_endpoint(endpoint: &str) -> Result<Option<SigningKey>, String> {
    if !client_config::use_signed_requests(endpoint) {
        return Ok(None);
    }
    let path = signing_identity_path()?;
    load_or_create_identity(&path)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Call `GetStatus` on deploy-server (same as `client status`); uses signing if previously paired.
pub fn verify_grpc_endpoint(endpoint: &str) -> Result<GrpcConnectResult, String> {
    validate_endpoint(endpoint)?;
    let endpoint = normalize_endpoint(endpoint);
    crate::tokio_runtime::block_on(async move {
        let status_check = async move {
            let ep = Endpoint::from_shared(endpoint.clone())
                .map_err(|e| format!("invalid endpoint: {e}"))?
                .connect_timeout(GRPC_STATUS_CONNECT_TIMEOUT)
                .timeout(GRPC_STATUS_RPC_TIMEOUT);
            let channel = match ep.connect().await {
                Ok(channel) => channel,
                Err(e) => return Err(format!("connect failed: {e}")),
            };
            let mut client = DeployServiceClient::new(channel);
            let pid = load_project_id();
            let mut req = Request::new(StatusRequest {
                project_id: pid.clone(),
            });
            if client_config::use_signed_requests(&endpoint) {
                let sk = load_or_create_identity(&signing_identity_path()?)
                    .map_err(|e| e.to_string())?;
                attach_auth_metadata(&mut req, &sk, "GetStatus", &pid, "")
                    .map_err(|e| e.to_string())?;
            }
            let r: StatusResponse = match client.get_status(req).await {
                Ok(resp) => resp.into_inner(),
                Err(e) => return Err(format!("GetStatus failed: {e}")),
            };
            Ok(grpc_connect_result(endpoint, r))
        };
        match tokio::time::timeout(GRPC_STATUS_TOTAL_TIMEOUT, status_check).await {
            Ok(result) => result,
            Err(_) => Err(format!(
                "gRPC status check timed out after {}s",
                GRPC_STATUS_TOTAL_TIMEOUT.as_secs()
            )),
        }
    })
}

/// GetStatus for a specific deploy `project_id` (uses saved gRPC endpoint).
/// Safe to `.await` from Tauri / existing Tokio runtime (unlike [`verify_grpc_status_for_project`]).
pub async fn verify_grpc_status_for_project_async(
    project_id: &str,
) -> Result<GrpcConnectResult, String> {
    let endpoint = load_endpoint().ok_or_else(|| "no saved endpoint".to_string())?;
    validate_endpoint(&endpoint)?;
    let endpoint = normalize_endpoint(&endpoint);
    let pid = deploy_core::normalize_project_id(project_id);

    let mut client = DeployServiceClient::connect(endpoint.clone())
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    let mut req = Request::new(StatusRequest {
        project_id: pid.clone(),
    });
    if client_config::use_signed_requests(&endpoint) {
        let sk = load_or_create_identity(&signing_identity_path()?).map_err(|e| e.to_string())?;
        attach_auth_metadata(&mut req, &sk, "GetStatus", &pid, "").map_err(|e| e.to_string())?;
    }
    let r: StatusResponse = client
        .get_status(req)
        .await
        .map_err(|e| format!("GetStatus failed: {e}"))?
        .into_inner();
    Ok(grpc_connect_result(endpoint, r))
}

/// GetStatus for a specific deploy `project_id` (uses saved gRPC endpoint).
///
/// Uses a dedicated runtime + `block_on` — **do not** call from async code that already runs on Tokio
/// (e.g. inside `async fn` Tauri commands); use [`verify_grpc_status_for_project_async`] instead.
pub fn verify_grpc_status_for_project(project_id: &str) -> Result<GrpcConnectResult, String> {
    let pid = deploy_core::normalize_project_id(project_id);
    {
        let cache = GRPC_VERIFY_CACHE.lock();
        if let Some((r, t)) = cache.get(&pid) {
            if t.elapsed() < GRPC_VERIFY_CACHE_TTL {
                return Ok(r.clone());
            }
        }
    }
    let result = crate::tokio_runtime::block_on(verify_grpc_status_for_project_async(project_id))?;
    GRPC_VERIFY_CACHE.lock().insert(pid, (result.clone(), Instant::now()));
    Ok(result)
}

pub fn save_endpoint(endpoint: &str) -> Result<(), String> {
    ensure_unified_client_config_migrated();
    validate_endpoint(endpoint)?;
    let endpoint = normalize_endpoint(endpoint);
    let existing = load_connection();
    let connection_kind = existing.as_ref().map(|c| c.connection_kind).unwrap_or(0);
    let project_id = existing
        .as_ref()
        .map(|c| c.project_id.as_str())
        .filter(|p| !p.trim().is_empty())
        .map(|p| deploy_core::normalize_project_id(p))
        .unwrap_or_else(default_project_id);

    let (paired, server_pubkey_b64) = if let Some(pk) = bookmark_pairing_pubkey_for_url(&endpoint) {
        (true, pk)
    } else {
        (false, String::new())
    };

    let body = StoredConnection {
        url: endpoint.clone(),
        server_pubkey_b64,
        paired,
        connection_kind,
        project_id,
    };
    save_canon_connection(&body)?;
    let _ = crate::bookmarks::upsert_bookmark(&endpoint, &endpoint);
    Ok(())
}

pub fn load_endpoint() -> Option<String> {
    ensure_unified_client_config_migrated();
    load_connection().and_then(|c| {
        let u = c.url.trim();
        if u.is_empty() {
            None
        } else {
            Some(u.to_string())
        }
    })
}

/// HTTP base for control-api (`/api/v1/...`), **not** the gRPC deploy-server URL.
/// Charts and REST host-stats series use this; gRPC stays on [`load_endpoint`].
pub fn load_control_api_base() -> Option<String> {
    let c = desktop_store::open().ok()?;
    let s: String = c
        .query_row(
            "SELECT control_api_base_url FROM connection WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(normalize_endpoint(t))
    }
}

/// Persist control-api HTTP base (e.g. `http://192.168.0.30:8080`). Empty string clears.
/// Changing the base URL clears a stored control-api JWT (session is host-specific).
pub fn set_control_api_base(url: &str) -> Result<(), String> {
    let t = url.trim();
    if !t.is_empty() {
        validate_endpoint(t)?;
    }
    let new_base = normalize_endpoint(t);
    let old = load_control_api_base();
    if old.as_ref().map(|s| s.as_str()) == Some(new_base.as_str()) {
        return Ok(());
    }
    set_control_api_base_with_mode(url, CONTROL_API_BASE_MODE_MANUAL)
}

fn set_control_api_base_with_mode(url: &str, mode: &str) -> Result<(), String> {
    let t = url.trim();
    if !t.is_empty() {
        validate_endpoint(t)?;
    }
    let new_base = normalize_endpoint(t);
    let old = load_control_api_base();
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE connection SET control_api_base_url = ?1, control_api_base_mode = ?2 WHERE id = 1",
        params![new_base.clone(), mode],
    )
    .map_err(|e| e.to_string())?;
    if old.as_ref().map(|s| s.as_str()) != Some(new_base.as_str()) {
        let _ = clear_control_api_jwt();
    }
    Ok(())
}

fn load_control_api_base_mode() -> Option<String> {
    let c = desktop_store::open().ok()?;
    let s: String = c
        .query_row(
            "SELECT control_api_base_mode FROM connection WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

pub fn mark_control_api_recent_restart(seconds: i64) -> Result<(), String> {
    let ttl = seconds.max(1);
    let until = now_ms().saturating_add(ttl.saturating_mul(1000));
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE connection SET control_api_recent_restart_until_ms = ?1 WHERE id = 1",
        params![until],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn control_api_recent_restart_hint() -> bool {
    let c = match desktop_store::open() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let until: i64 = match c.query_row(
        "SELECT control_api_recent_restart_until_ms FROM connection WHERE id = 1",
        [],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(_) => return false,
    };
    until > now_ms()
}

/// Stored JWT from control-api `POST /api/v1/auth/login` and absolute expiry (ms since epoch).
pub(crate) fn save_control_api_jwt(token: &str, expires_at_ms: i64) -> Result<(), String> {
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE connection SET control_api_jwt = ?1, control_api_jwt_expires_at_ms = ?2 WHERE id = 1",
        params![token.trim(), expires_at_ms],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn clear_control_api_jwt() -> Result<(), String> {
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE connection SET control_api_jwt = '', control_api_jwt_expires_at_ms = 0 WHERE id = 1",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn load_control_api_jwt() -> Option<(String, i64)> {
    let c = desktop_store::open().ok()?;
    c.query_row(
        "SELECT control_api_jwt, control_api_jwt_expires_at_ms FROM connection WHERE id = 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )
    .ok()
}

/// Active deploy project id (persisted with the gRPC connection).
pub fn load_project_id() -> String {
    ensure_unified_client_config_migrated();
    load_connection()
        .map(|c| {
            let p = c.project_id.trim();
            if p.is_empty() {
                "default".to_string()
            } else {
                deploy_core::normalize_project_id(p)
            }
        })
        .unwrap_or_else(|| "default".to_string())
}

/// Set active project id (requires an existing saved connection file).
pub fn set_active_project(project_id: String) -> Result<(), String> {
    ensure_unified_client_config_migrated();
    deploy_core::validate_project_id(&project_id).map_err(|e| e.to_string())?;
    let mut conn = load_connection().ok_or_else(|| "save a gRPC connection first".to_string())?;
    conn.project_id = deploy_core::normalize_project_id(&project_id);
    save_canon_connection(&conn)
}

/// Switch active connection to saved URL and verify gRPC (GetStatus).
pub fn activate_bookmark_url(url: &str) -> Result<GrpcConnectResult, String> {
    save_endpoint(url)?;
    let ep = load_endpoint().ok_or_else(|| "failed to save endpoint".to_string())?;
    verify_grpc_endpoint(&ep)
}

/// Add a saved server from a plain gRPC URL, minimal `{"url":…}` / `{"endpoint":…}`, `export GRPC_ENDPOINT=…`, or full install JSON with `token` + `url` (pairing pubkey stored on the bookmark).
pub fn add_bookmark_from_input(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("input is empty".into());
    }
    if t.starts_with('{') && install_json_has_token(t).map_err(|e| e.to_string())? {
        let b = ConnectionBundle::parse(t).map_err(|e| e.to_string())?;
        validate_endpoint(&b.url)?;
        let url = normalize_endpoint(&b.url);
        let id = crate::bookmarks::upsert_bookmark(&url, &url)?;
        crate::bookmarks::set_bookmark_pairing(&url, b.server_pubkey_b64)?;
        return Ok(id);
    }
    let url = parse_grpc_endpoint_from_bundle(raw)?;
    crate::bookmarks::upsert_bookmark(&url, &url)
}

pub fn clear_endpoint() -> Result<(), String> {
    if let Some(p) = client_config::connection_path() {
        let _ = std::fs::remove_file(p);
    }
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE connection SET url = '', server_pubkey_b64 = NULL, paired = 0, control_api_base_url = '', control_api_base_mode = 'auto', control_api_direct_url = '', control_api_jwt = '', control_api_jwt_expires_at_ms = 0, control_api_recent_restart_until_ms = 0 WHERE id = 1",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Parse install JSON, run `Pair`, verify server, save connection + identity.
pub fn connect_from_bundle(bundle: &str) -> Result<GrpcConnectResult, String> {
    ensure_unified_client_config_migrated();
    let t = bundle.trim();
    if !install_json_has_token(t)? {
        let ep = parse_grpc_endpoint_from_bundle(bundle)?;
        let res = verify_grpc_endpoint(&ep)?;
        save_endpoint(&res.endpoint)?;
        return Ok(res);
    }

    let b = ConnectionBundle::parse(t).map_err(|e| e.to_string())?;
    let pairing = b
        .pairing_code
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or("bundle must include pairing code from the server")?;

    let project_id = load_connection()
        .map(|c| c.project_id)
        .filter(|p| !p.trim().is_empty())
        .map(|p| deploy_core::normalize_project_id(&p))
        .unwrap_or_else(default_project_id);

    let id_path = signing_identity_path()?;
    let sk = load_or_create_identity(&id_path).map_err(|e| e.to_string())?;
    let client_pub = pubkey_b64_url(&sk);
    let ts_ms = now_unix_ms();
    let nonce = format!("{:016x}", OsRng.next_u64());
    let msg = pair_request_canonical(&client_pub, &b.server_pubkey_b64, ts_ms, &nonce, &pairing);
    let client_sig = deploy_auth::sign_bytes(&sk, &msg);

    crate::tokio_runtime::block_on(async move {
        let mut client = DeployServiceClient::connect(b.url.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let resp = client
            .pair(Request::new(PairRequest {
                client_public_key_b64: client_pub.clone(),
                timestamp_ms: ts_ms,
                nonce: nonce.clone(),
                pairing_code: pairing,
                client_signature_b64: client_sig,
            }))
            .await
            .map_err(|e| format!("Pair failed: {e}"))?
            .into_inner();

        verify_pair_response(
            &b.server_pubkey_b64,
            &client_pub,
            ts_ms,
            &nonce,
            &resp.server_signature_b64,
        )
        .map_err(|e| format!("server identity check failed: {e}"))?;

        let body = StoredConnection {
            url: normalize_endpoint(&b.url),
            server_pubkey_b64: b.server_pubkey_b64.clone(),
            paired: true,
            connection_kind: 0,
            project_id: project_id.clone(),
        };
        save_canon_connection(&body).map_err(|e| e.to_string())?;

        let mut client = DeployServiceClient::connect(b.url.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let pid = body.project_id.clone();
        let mut req = Request::new(StatusRequest {
            project_id: pid.clone(),
        });
        attach_auth_metadata(&mut req, &sk, "GetStatus", &pid, "").map_err(|e| e.to_string())?;
        let r: StatusResponse = client
            .get_status(req)
            .await
            .map_err(|e| format!("GetStatus failed: {e}"))?
            .into_inner();

        let endpoint = normalize_endpoint(&b.url);
        let _ = crate::bookmarks::upsert_bookmark(&endpoint, &endpoint);
        let _ = crate::bookmarks::set_bookmark_pairing(&endpoint, b.server_pubkey_b64.clone());

        Ok(grpc_connect_result(endpoint, r))
    })
}

/// Copy deploy pairing from `old_url` to `new_url`, persist [`StoredConnection`], then verify with `GetStatus`.
pub fn migrate_grpc_public_endpoint(
    old_url: &str,
    new_url: &str,
) -> Result<GrpcConnectResult, String> {
    ensure_unified_client_config_migrated();
    let old_n = normalize_endpoint(old_url);
    let new_n = normalize_endpoint(new_url);
    validate_endpoint(&old_n)?;
    validate_endpoint(&new_n)?;
    if old_n == new_n {
        return verify_grpc_endpoint(&new_n);
    }

    let pk = bookmark_pairing_pubkey_for_url(&old_n).or_else(|| {
        load_connection().and_then(|c| {
            if normalize_endpoint(&c.url) == old_n
                && c.paired
                && !c.server_pubkey_b64.trim().is_empty()
            {
                Some(c.server_pubkey_b64)
            } else {
                None
            }
        })
    }).ok_or_else(|| {
        "no paired identity for the current address; pair again or add a bookmark for the old URL"
            .to_string()
    })?;

    crate::bookmarks::migrate_bookmark_url_keep_pairing(&old_n, &new_n, &pk)?;

    let conn = load_connection();
    let project_id = conn
        .as_ref()
        .map(|c| deploy_core::normalize_project_id(c.project_id.trim()))
        .unwrap_or_else(default_project_id);
    let connection_kind = conn.as_ref().map(|c| c.connection_kind).unwrap_or(0);

    let body = StoredConnection {
        url: new_n.clone(),
        server_pubkey_b64: pk,
        paired: true,
        connection_kind,
        project_id,
    };
    save_canon_connection(&body)?;
    verify_grpc_endpoint(&new_n)
}

#[cfg(test)]
mod tests {
    use super::parse_grpc_endpoint_from_bundle;

    #[test]
    fn parse_minimal_url_json() {
        assert_eq!(
            parse_grpc_endpoint_from_bundle(r#"{"url":"http://127.0.0.1:50051"}"#).unwrap(),
            "http://127.0.0.1:50051"
        );
    }

    #[test]
    fn parse_endpoint_alias_json() {
        assert_eq!(
            parse_grpc_endpoint_from_bundle(r#"{"endpoint":"http://[::1]:50051/"}"#).unwrap(),
            "http://[::1]:50051"
        );
    }

    #[test]
    fn parse_install_bundle_extracts_url() {
        let u = parse_grpc_endpoint_from_bundle(
            r#"{"token":"dGVzdA","url":"http://example.test:50051","pairing":"abc"}"#,
        )
        .unwrap();
        assert_eq!(u, "http://example.test:50051");
    }

    #[test]
    fn parse_json_object_without_url_errors() {
        assert!(parse_grpc_endpoint_from_bundle("{}").is_err());
    }
}
