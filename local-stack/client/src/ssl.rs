//! `pirate ssl` — gRPC to deploy-server SSL/Certbot APIs.

use crate::config::{normalize_endpoint, use_signed_requests};
use crate::config::load_or_create_identity;
use deploy_auth::attach_auth_metadata;
use deploy_proto::deploy::{
    SslCheckAndRenewRequest, SslCreateRequest, SslDomainSpec, SslMode, SslStatusRequest, SslUpdateRequest,
    SslUpdateSelector,
};
use deploy_proto::DeployServiceClient;
use tonic::Request;

fn ssl_mode_from_cli(s: Option<&str>) -> SslMode {
    match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
        Some("nginx") => SslMode::Nginx,
        Some("standalone") => SslMode::Standalone,
        Some("webroot") => SslMode::Webroot,
        Some("dns") => SslMode::Dns,
        _ => SslMode::Unspecified,
    }
}

fn status_label(code: i32) -> &'static str {
    use deploy_proto::deploy::SslCertStatus;
    match code {
        x if x == SslCertStatus::Valid as i32 => "valid",
        x if x == SslCertStatus::ExpiringSoon as i32 => "expiring_soon",
        x if x == SslCertStatus::Expired as i32 => "expired",
        x if x == SslCertStatus::Error as i32 => "error",
        _ => "unknown",
    }
}

fn status_icon(code: i32) -> &'static str {
    use deploy_proto::deploy::SslCertStatus;
    match code {
        x if x == SslCertStatus::Valid as i32 => "[ok]",
        x if x == SslCertStatus::ExpiringSoon as i32 => "[!]",
        x if x == SslCertStatus::Expired as i32 => "[x]",
        x if x == SslCertStatus::Error as i32 => "[!]",
        _ => "[?]",
    }
}

pub async fn run_ssl_create(
    endpoint: &str,
    project_id: &str,
    domains: &[String],
    mode: Option<&str>,
    webroot: Option<&str>,
    dry_run: bool,
    staging: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if domains.is_empty() {
        return Err("pass at least one --domain".into());
    }
    let endpoint = normalize_endpoint(endpoint);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("endpoint must start with http:// or https://".into());
    }
    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
    let spec = SslDomainSpec {
        domains: domains.to_vec(),
        mode: ssl_mode_from_cli(mode) as i32,
        dry_run,
        staging,
        webroot_path: webroot.unwrap_or("").to_string(),
        project_id: project_id.to_string(),
    };
    let mut req = Request::new(SslCreateRequest { spec: Some(spec) });
    if use_signed_requests(&endpoint) {
        let sk = load_or_create_identity()?;
        attach_auth_metadata(&mut req, &sk, "SslCreate", project_id, "")?;
    }
    let r = client.ssl_create(req).await?.into_inner();
    println!("status={}", r.status);
    if let Some(c) = r.cert {
        print_cert_line(&c, true);
    }
    for line in r.log_lines {
        eprintln!("{line}");
    }
    Ok(())
}

pub async fn run_ssl_status(endpoint: &str, project_id: &str, verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = normalize_endpoint(endpoint);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("endpoint must start with http:// or https://".into());
    }
    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
    let mut req = Request::new(SslStatusRequest {
        include_paths: verbose,
        project_id: project_id.to_string(),
    });
    if use_signed_requests(&endpoint) {
        let sk = load_or_create_identity()?;
        attach_auth_metadata(&mut req, &sk, "SslStatus", project_id, "")?;
    }
    let r = client.ssl_status(req).await?.into_inner();
    println!("threshold_days={}", r.threshold_days);
    println!(
        "{:6} {:32} {:>12} {:>16} {}",
        "stat", "primary", "expiry_ms", "status", "domains"
    );
    for c in &r.certs {
        let doms = c.domains.join(",");
        let exp = c.expiry_unix_ms;
        println!(
            "{:6} {:32} {:>12} {:>16} {}",
            status_icon(c.status),
            truncate(&c.primary_domain, 32),
            exp,
            status_label(c.status),
            truncate(&doms, 48),
        );
        if verbose {
            if !c.live_path.is_empty() {
                println!("       live_path={}", c.live_path);
            }
            if !c.cert_name.is_empty() {
                println!("       cert_name={}", c.cert_name);
            }
            if !c.last_error.is_empty() {
                println!("       last_error={}", c.last_error);
            }
        }
    }
    Ok(())
}

pub async fn run_ssl_update(
    endpoint: &str,
    project_id: &str,
    domain: Option<&str>,
    glob: Option<&str>,
    regex: Option<&str>,
    dry_run: bool,
    staging: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let n = [domain.is_some(), glob.is_some(), regex.is_some()]
        .iter()
        .filter(|x| **x)
        .count();
    if n != 1 {
        return Err("specify exactly one of --domain, --ur, or --regex".into());
    }
    let endpoint = normalize_endpoint(endpoint);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("endpoint must start with http:// or https://".into());
    }
    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
    let selector = Some(SslUpdateSelector {
        exact_domain: domain.unwrap_or("").to_string(),
        glob_pattern: glob.unwrap_or("").to_string(),
        regex: regex.unwrap_or("").to_string(),
    });
    let mut req = Request::new(SslUpdateRequest {
        selector,
        mode: SslMode::Unspecified as i32,
        dry_run,
        staging,
        project_id: project_id.to_string(),
    });
    if use_signed_requests(&endpoint) {
        let sk = load_or_create_identity()?;
        attach_auth_metadata(&mut req, &sk, "SslUpdate", project_id, "")?;
    }
    let r = client.ssl_update(req).await?.into_inner();
    println!("status={}", r.status);
    for c in r.updated {
        print_cert_line(&c, true);
    }
    for line in r.log_lines {
        eprintln!("{line}");
    }
    Ok(())
}

pub async fn run_ssl_check_and_renew(
    endpoint: &str,
    project_id: &str,
    force_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = normalize_endpoint(endpoint);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("endpoint must start with http:// or https://".into());
    }
    let mut client = DeployServiceClient::connect(endpoint.clone()).await?;
    let mut req = Request::new(SslCheckAndRenewRequest {
        force_all,
        project_id: project_id.to_string(),
    });
    if use_signed_requests(&endpoint) {
        let sk = load_or_create_identity()?;
        attach_auth_metadata(
            &mut req,
            &sk,
            "SslCheckAndRenew",
            project_id,
            "",
        )?;
    }
    let r = client.ssl_check_and_renew(req).await?.into_inner();
    println!(
        "status={} checked={} attempted_renew={} failed={}",
        r.status, r.checked, r.attempted_renew, r.failed
    );
    for line in r.log_lines {
        eprintln!("{line}");
    }
    Ok(())
}

fn print_cert_line(c: &deploy_proto::deploy::SslCertRecord, _verbose: bool) {
    println!(
        "primary={} expiry_ms={} status={} path={} cert_name={} err={}",
        c.primary_domain,
        c.expiry_unix_ms,
        status_label(c.status),
        c.live_path,
        c.cert_name,
        c.last_error
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_maps() {
        use deploy_proto::deploy::SslCertStatus;
        assert_eq!(status_label(SslCertStatus::Valid as i32), "valid");
    }
}
