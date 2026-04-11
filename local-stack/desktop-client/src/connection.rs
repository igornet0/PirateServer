//! Connection bundle parsing, pairing, and signed `GetStatus`.

use deploy_auth::{
    attach_auth_metadata, load_or_create_identity, pair_request_canonical, pubkey_b64_url,
    verify_pair_response, ConnectionBundle, now_unix_ms,
};
use deploy_proto::deploy::{PairRequest, StatusRequest, StatusResponse};
use ed25519_dalek::SigningKey;
use deploy_proto::DeployServiceClient;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tonic::Request;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcConnectResult {
    pub endpoint: String,
    pub current_version: String,
    pub state: String,
}

fn config_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("grpc_connection.json")
}

fn identity_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("identity.json")
}

fn default_project_id() -> String {
    "default".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredConnection {
    /// New field name; `endpoint` accepted for backward compatibility.
    #[serde(alias = "endpoint")]
    url: String,
    #[serde(default)]
    server_pubkey_b64: Option<String>,
    #[serde(default)]
    paired: bool,
    #[serde(default = "default_project_id")]
    project_id: String,
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

/// Extract URL from install JSON or legacy `export GRPC_ENDPOINT=…` / single URL line.
pub fn parse_grpc_endpoint_from_bundle(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("paste is empty".into());
    }

    if t.starts_with('{') {
        let b = ConnectionBundle::parse(t).map_err(|e| e.to_string())?;
        return Ok(normalize_endpoint(&b.url));
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
        let rest = line
            .strip_prefix("export ")
            .unwrap_or(line)
            .trim();
        if let Some(val) = rest.strip_prefix("GRPC_ENDPOINT=") {
            let v = val
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim();
            if v.starts_with("http://") || v.starts_with("https://") {
                return Ok(normalize_endpoint(v));
            }
        }
    }

    Err(
        "expected JSON bundle with token/url/pairing, or export GRPC_ENDPOINT=…, or a single http(s) URL"
            .into(),
    )
}

fn load_stored() -> Option<StoredConnection> {
    let path = config_path();
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn use_signed_for_endpoint(endpoint: &str) -> bool {
    load_stored()
        .map(|s| s.paired && normalize_endpoint(&s.url) == normalize_endpoint(endpoint))
        .unwrap_or(false)
}

/// When the saved connection is paired, returns the client signing key for gRPC auth.
pub fn load_signing_key_for_endpoint(endpoint: &str) -> Result<Option<SigningKey>, String> {
    let endpoint = normalize_endpoint(endpoint);
    if !use_signed_for_endpoint(&endpoint) {
        return Ok(None);
    }
    load_or_create_identity(&identity_path())
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Call `GetStatus` on deploy-server (same as `client status`); uses signing if previously paired.
pub fn verify_grpc_endpoint(endpoint: &str) -> Result<GrpcConnectResult, String> {
    validate_endpoint(endpoint)?;
    let endpoint = normalize_endpoint(endpoint);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let pid = load_project_id();
        let mut req = Request::new(StatusRequest {
            project_id: pid.clone(),
        });
        if use_signed_for_endpoint(&endpoint) {
            let sk = load_or_create_identity(&identity_path()).map_err(|e| e.to_string())?;
            attach_auth_metadata(&mut req, &sk, "GetStatus", &pid, "").map_err(|e| e.to_string())?;
        }
        let r: StatusResponse = client
            .get_status(req)
            .await
            .map_err(|e| format!("GetStatus failed: {e}"))?
            .into_inner();
        Ok(GrpcConnectResult {
            endpoint,
            current_version: r.current_version,
            state: r.state,
        })
    })
}

pub fn save_endpoint(endpoint: &str) -> Result<(), String> {
    validate_endpoint(endpoint)?;
    let endpoint = normalize_endpoint(endpoint);
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let project_id = load_stored()
        .map(|s| s.project_id)
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(default_project_id);
    let body = StoredConnection {
        url: endpoint.clone(),
        server_pubkey_b64: None,
        paired: false,
        project_id,
    };
    let json = serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    let _ = crate::bookmarks::upsert_bookmark(&endpoint, &endpoint);
    Ok(())
}

pub fn load_endpoint() -> Option<String> {
    load_stored().map(|s| s.url)
}

/// Active deploy project id (persisted with the gRPC connection).
pub fn load_project_id() -> String {
    load_stored()
        .map(|s| {
            let p = s.project_id.trim();
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
    deploy_core::validate_project_id(&project_id).map_err(|e| e.to_string())?;
    let path = config_path();
    let mut s = load_stored().ok_or_else(|| "save a gRPC connection first".to_string())?;
    s.project_id = deploy_core::normalize_project_id(&project_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Switch active connection to saved URL and verify gRPC (GetStatus).
pub fn activate_bookmark_url(url: &str) -> Result<GrpcConnectResult, String> {
    save_endpoint(url)?;
    let ep = load_endpoint().ok_or_else(|| "failed to save endpoint".to_string())?;
    verify_grpc_endpoint(&ep)
}

pub fn clear_endpoint() -> Result<(), String> {
    let path = config_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Parse install JSON, run `Pair`, verify server, save connection + identity.
pub fn connect_from_bundle(bundle: &str) -> Result<GrpcConnectResult, String> {
    let t = bundle.trim();
    if !t.starts_with('{') {
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

    let sk = load_or_create_identity(&identity_path()).map_err(|e| e.to_string())?;
    let client_pub = pubkey_b64_url(&sk);
    let ts_ms = now_unix_ms();
    let nonce = format!("{:016x}", OsRng.next_u64());
    let msg = pair_request_canonical(&client_pub, &b.server_pubkey_b64, ts_ms, &nonce, &pairing);
    let client_sig = deploy_auth::sign_bytes(&sk, &msg);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    rt.block_on(async move {
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

        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let body = StoredConnection {
            url: normalize_endpoint(&b.url),
            server_pubkey_b64: Some(b.server_pubkey_b64.clone()),
            paired: true,
            project_id: load_stored()
                .map(|s| s.project_id)
                .filter(|p| !p.trim().is_empty())
                .unwrap_or_else(default_project_id),
        };
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

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

        Ok(GrpcConnectResult {
            endpoint,
            current_version: r.current_version,
            state: r.state,
        })
    })
}
