//! gRPC `Ssl*` calls for desktop UI (same auth as `GetHostStats`).
//!
//! **Manual e2e (phase 1):** connect gRPC to a server, open the matching bookmark’s
//! Server settings → **SSL**; **Refresh** lists certs; **Create** with dry-run + staging;
//! **Check and renew** without `force_all`; **SSL scheduler** block saves host env with JWT.

use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::{
    SslCheckAndRenewRequest, SslCreateRequest, SslDomainSpec, SslStatusRequest, SslUpdateRequest,
    SslUpdateSelector,
};
use deploy_proto::DeployServiceClient;
use serde_json::{json, Value};
use tonic::Request;

use crate::connection::load_signing_key_for_endpoint;
use deploy_client::config::normalize_endpoint;
use deploy_core::normalize_project_id;

fn attach_if_paired<T>(
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

fn post_check_to_v(p: &deploy_proto::deploy::SslPostCheckResult) -> Value {
    let details: Vec<Value> = p
        .details
        .iter()
        .map(|d| {
            json!({
                "step": d.step,
                "ok": d.ok,
                "message": d.message,
            })
        })
        .collect();
    json!({
        "nginx_test_ok": p.nginx_test_ok,
        "reload_ok": p.reload_ok,
        "tls_handshake_ok": p.tls_handshake_ok,
        "hostname_match_ok": p.hostname_match_ok,
        "chain_ok": p.chain_ok,
        "upstream_health_ok": p.upstream_health_ok,
        "rollback_performed": p.rollback_performed,
        "summary": p.summary,
        "details": details,
        "http_status": p.http_status,
        "classified_error": p.classified_error,
        "probe_host": p.probe_host,
        "curl_exit": p.curl_exit,
    })
}

fn cert_to_v(c: &deploy_proto::deploy::SslCertRecord) -> Value {
    json!({
        "primary_domain": c.primary_domain,
        "domains": c.domains,
        "expiry_unix_ms": c.expiry_unix_ms,
        "status": c.status,
        "live_path": c.live_path,
        "last_error": c.last_error,
        "updated_at_ms": c.updated_at_ms,
        "cert_name": c.cert_name,
    })
}

/// JSON: `{ "certs": [...], "threshold_days": n }` or error from tonic.
pub fn ssl_status_json(grpc_url: &str, project_id: &str) -> Result<String, String> {
    let endpoint = normalize_endpoint(grpc_url);
    if endpoint.is_empty() {
        return Err("empty gRPC URL".into());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let pid = normalize_project_id(project_id);
        let mut req = Request::new(SslStatusRequest {
            include_paths: true,
            project_id: pid.clone(),
        });
        attach_if_paired(&mut req, &endpoint, "SslStatus", &pid)?;
        let r = client
            .ssl_status(req)
            .await
            .map_err(|e| format!("SslStatus failed: {e}"))?
            .into_inner();
        let certs: Vec<Value> = r.certs.iter().map(cert_to_v).collect();
        let v = json!({
            "certs": certs,
            "threshold_days": r.threshold_days,
        });
        serde_json::to_string(&v).map_err(|e| e.to_string())
    })
}

/// `mode`: 0=auto/env, 1=nginx, 2=standalone, 3=webroot, 4=dns
pub fn ssl_create_json(
    grpc_url: &str,
    project_id: &str,
    domains: Vec<String>,
    mode: i32,
    webroot_path: &str,
    dry_run: bool,
    staging: bool,
) -> Result<String, String> {
    let endpoint = normalize_endpoint(grpc_url);
    if endpoint.is_empty() {
        return Err("empty gRPC URL".into());
    }
    if domains.is_empty() {
        return Err("at least one domain is required".into());
    }
    let pid = normalize_project_id(project_id);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let spec = SslDomainSpec {
            domains,
            mode,
            dry_run,
            staging,
            webroot_path: webroot_path.to_string(),
            project_id: pid.clone(),
        };
        let mut req = Request::new(SslCreateRequest {
            spec: Some(spec),
        });
        attach_if_paired(&mut req, &endpoint, "SslCreate", &pid)?;
        let r = client
            .ssl_create(req)
            .await
            .map_err(|e| format!("SslCreate failed: {e}"))?
            .into_inner();
        let out = json!({
            "status": r.status,
            "cert": r.cert.as_ref().map(cert_to_v),
            "log_lines": r.log_lines,
            "post_check": r.post_check.as_ref().map(post_check_to_v),
        });
        serde_json::to_string(&out).map_err(|e| e.to_string())
    })
}

/// Exactly one of `exact_domain`, `glob_pattern`, `regex` should be non-empty.
pub fn ssl_update_json(
    grpc_url: &str,
    project_id: &str,
    exact_domain: &str,
    glob_pattern: &str,
    regex: &str,
    dry_run: bool,
) -> Result<String, String> {
    let endpoint = normalize_endpoint(grpc_url);
    if endpoint.is_empty() {
        return Err("empty gRPC URL".into());
    }
    let pid = normalize_project_id(project_id);
    let sel_count = [exact_domain, glob_pattern, regex]
        .iter()
        .filter(|s| !s.is_empty())
        .count();
    if sel_count != 1 {
        return Err("specify exactly one of exact_domain, glob_pattern, regex".into());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let selector = Some(SslUpdateSelector {
            exact_domain: exact_domain.to_string(),
            glob_pattern: glob_pattern.to_string(),
            regex: regex.to_string(),
        });
        let mut req = Request::new(SslUpdateRequest {
            selector,
            mode: 0,
            dry_run,
            staging: false,
            project_id: pid.clone(),
        });
        attach_if_paired(&mut req, &endpoint, "SslUpdate", &pid)?;
        let r = client
            .ssl_update(req)
            .await
            .map_err(|e| format!("SslUpdate failed: {e}"))?
            .into_inner();
        let updated: Vec<Value> = r.updated.iter().map(cert_to_v).collect();
        let v = json!({
            "status": r.status,
            "updated": updated,
            "log_lines": r.log_lines,
            "post_check": r.post_check.as_ref().map(post_check_to_v),
        });
        serde_json::to_string(&v).map_err(|e| e.to_string())
    })
}

pub fn ssl_check_and_renew_json(
    grpc_url: &str,
    project_id: &str,
    force_all: bool,
) -> Result<String, String> {
    let endpoint = normalize_endpoint(grpc_url);
    if endpoint.is_empty() {
        return Err("empty gRPC URL".into());
    }
    let pid = normalize_project_id(project_id);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let mut client = DeployServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect failed: {e}"))?;
        let mut req = Request::new(SslCheckAndRenewRequest {
            force_all,
            project_id: pid.clone(),
        });
        attach_if_paired(&mut req, &endpoint, "SslCheckAndRenew", &pid)?;
        let r = client
            .ssl_check_and_renew(req)
            .await
            .map_err(|e| format!("SslCheckAndRenew failed: {e}"))?
            .into_inner();
        let v = json!({
            "status": r.status,
            "checked": r.checked,
            "attempted_renew": r.attempted_renew,
            "failed": r.failed,
            "log_lines": r.log_lines,
            "post_check": r.post_check.as_ref().map(post_check_to_v),
        });
        serde_json::to_string(&v).map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cert_json_shape() {
        let c = deploy_proto::deploy::SslCertRecord {
            primary_domain: "a.com".to_string(),
            domains: vec!["a.com".to_string()],
            expiry_unix_ms: 1,
            status: 1,
            live_path: "/x".to_string(),
            last_error: "".to_string(),
            updated_at_ms: 2,
            cert_name: "a.com".to_string(),
        };
        let v = cert_to_v(&c);
        assert_eq!(v["primary_domain"], "a.com");
    }
}
