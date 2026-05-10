//! Orchestration: pre-checks, DB, Certbot, expiry classification.

use super::certbot;
use super::config::{SslModeResolved, SslProcessConfig, load_ssl_config};
use deploy_db::{DbStore, SslCertificateRow};
use deploy_proto::deploy::{
    SslCertRecord, SslCertStatus, SslCheckAndRenewRequest, SslCheckAndRenewResponse, SslCreateRequest,
    SslCreateResponse, SslMode, SslStatusRequest, SslStatusResponse, SslUpdateRequest, SslUpdateResponse,
    SslUpdateSelector,
};
use super::postcheck::{resolve_ssl_probe_host, run_post_check_after_cert};
use regex::Regex;
use std::path::Path;
use std::sync::Arc;
use tokio::net;

pub struct SslService {
    db: Arc<DbStore>,
    config: SslProcessConfig,
}

fn ssl_mode_from_i32(i: i32) -> SslMode {
    match i {
        0 => SslMode::Unspecified,
        1 => SslMode::Nginx,
        2 => SslMode::Standalone,
        3 => SslMode::Webroot,
        4 => SslMode::Dns,
        _ => SslMode::Unspecified,
    }
}

/// Parse `notAfter=Mar  8 00:00:00 2026 GMT` from `openssl x509 -noout -enddate`.
pub(crate) fn parse_not_after_millis(openssl_out: &str) -> Option<i64> {
    let t = openssl_out
        .lines()
        .next()?
        .strip_prefix("notAfter=")
        .map(str::trim)?;
    let t = t.strip_suffix(" GMT")?.trim();
    let parts: Vec<&str> = t.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let joined = format!("{} {} {} {}", parts[0], parts[1], parts[2], parts[3]);
    chrono::NaiveDateTime::parse_from_str(&joined, "%b %d %H:%M:%S %Y")
        .ok()
        .map(|n| n.and_utc().timestamp_millis())
}

impl SslService {
    pub fn new(db: Arc<DbStore>) -> Self {
        Self {
            db,
            config: load_ssl_config(),
        }
    }

    #[allow(dead_code)]
    pub fn with_config(db: Arc<DbStore>, config: SslProcessConfig) -> Self {
        Self { db, config }
    }

    pub fn validate_domain_syntax(host: &str) -> Result<(), String> {
        let h = host.trim();
        if h.is_empty() || h.len() > 253 {
            return Err("invalid domain length".into());
        }
        let core = h.strip_prefix("*.").unwrap_or(h);
        for label in core.split('.') {
            if label.is_empty() || label.len() > 63 {
                return Err("invalid label".into());
            }
            let ok = label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !ok {
                return Err("invalid label characters".into());
            }
        }
        Ok(())
    }

    pub async fn dns_resolves(&self, host: &str) -> Result<(), String> {
        let probe = host.strip_prefix("*.").unwrap_or(host);
        let mut addrs = net::lookup_host((probe, 0u16))
            .await
            .map_err(|e| format!("DNS {probe}: {e}"))?;
        if addrs.next().is_none() {
            return Err(format!("DNS {probe}: no addresses"));
        }
        Ok(())
    }

    fn resolve_mode(&self, proto_mode: SslMode) -> Result<SslModeResolved, String> {
        if proto_mode == SslMode::Unspecified || (proto_mode as i32) == 0 {
            return Ok(self.config.mode);
        }
        match proto_mode {
            SslMode::Nginx => Ok(SslModeResolved::Nginx),
            SslMode::Standalone => Ok(SslModeResolved::Standalone),
            SslMode::Webroot => Ok(SslModeResolved::Webroot),
            SslMode::Dns => Ok(SslModeResolved::Dns),
            SslMode::Unspecified => Ok(self.config.mode),
        }
    }

    /// Read notAfter (ms) from PEM via openssl (must exist on deploy host).
    pub async fn read_expiry_unix_ms(&self, fullchain: &Path) -> Option<i64> {
        let p = fullchain.to_path_buf();
        let o = tokio::process::Command::new("openssl")
            .args([
                "x509",
                "-in",
                p.to_str()?,
                "-noout",
                "-enddate",
            ])
            .output()
            .await
            .ok()?;
        if !o.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&o.stdout);
        parse_not_after_millis(&s)
    }

    /// Sync read (tests / sync paths).
    pub fn read_expiry_unix_ms_sync(fullchain: &Path) -> Option<i64> {
        let o = std::process::Command::new("openssl")
            .args([
                "x509",
                "-in",
                fullchain.to_str()?,
                "-noout",
                "-enddate",
            ])
            .output()
            .ok()?;
        if !o.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&o.stdout);
        parse_not_after_millis(&s)
    }

    fn classify(&self, expiry_ms: i64) -> (String, i32) {
        let th = i64::from(self.config.expiry_threshold_days.max(0)) * 86_400_000;
        let now = chrono::Utc::now().timestamp_millis();
        if expiry_ms <= 0 {
            return ("error".to_string(), SslCertStatus::Error as i32);
        }
        if expiry_ms < now {
            return ("expired".to_string(), SslCertStatus::Expired as i32);
        }
        if expiry_ms < now + th {
            return (
                "expiring_soon".to_string(),
                SslCertStatus::ExpiringSoon as i32,
            );
        }
        ("valid".to_string(), SslCertStatus::Valid as i32)
    }

    fn row_to_proto(
        &self,
        r: &SslCertificateRow,
        include_path: bool,
    ) -> SslCertRecord {
        let expiry = r.expiry_utc_ms;
        let (_st, st_enum) = self.classify(expiry);
        let code = if r.status == "error" {
            SslCertStatus::Error as i32
        } else {
            st_enum
        };
        SslCertRecord {
            primary_domain: r.primary_domain.clone(),
            domains: r.domains.clone(),
            expiry_unix_ms: expiry,
            status: code,
            live_path: if include_path {
                r.live_path.clone()
            } else {
                String::new()
            },
            last_error: r.last_error.clone().unwrap_or_default(),
            updated_at_ms: r.updated_at.timestamp_millis(),
            cert_name: r.cert_name.clone(),
        }
    }

    pub async fn create(&self, req: SslCreateRequest) -> Result<SslCreateResponse, String> {
        if !self.config.provider.eq_ignore_ascii_case("certbot") {
            return Err("only SSL_PROVIDER=certbot is supported in this build".into());
        }
        let spec = req.spec.as_ref().ok_or("missing SslDomainSpec")?;
        let email = if spec.dry_run {
            self.config
                .email
                .as_deref()
                .unwrap_or("dryrun@example.invalid")
        } else {
            self.config
                .email
                .as_deref()
                .ok_or("set SSL_EMAIL for certbot (or use --dry-run on client)")?
        };
        for d in &spec.domains {
            Self::validate_domain_syntax(d)?;
            self.dns_resolves(d).await?;
        }
        if spec.domains.is_empty() {
            return Err("at least one domain is required".into());
        }
        let mode = self.resolve_mode(ssl_mode_from_i32(spec.mode))?;
        if matches!(mode, SslModeResolved::Webroot) {
            if spec.webroot_path.trim().is_empty() && self.config.webroot_path.is_none() {
                return Err("webroot: set SSL_WEBROOT or spec.webroot_path".into());
            }
        }
        let web = if spec.webroot_path.is_empty() {
            None
        } else {
            Some(spec.webroot_path.as_str())
        };
        let primary = spec.domains[0].clone();
        let run = certbot::certonly(
            &self.config,
            email,
            &spec.domains,
            mode,
            web,
            spec.dry_run,
            spec.staging,
        )
        .await?;
        let mut log_lines = vec![format!("[certbot status={}]", run.status)];
        for line in run.stdout.lines() {
            log_lines.push(line.to_string());
        }
        for line in run.stderr.lines() {
            log_lines.push(format!("[stderr] {line}"));
        }
        if run.status != 0 {
            return Err(format!("certbot failed: {}", run.stderr));
        }
        if spec.dry_run {
            return Ok(SslCreateResponse {
                status: "dry_run_ok".to_string(),
                cert: None,
                log_lines,
                post_check: None,
            });
        }
        let live = std::path::PathBuf::from("/etc/letsencrypt/live").join(&primary);
        let fullchain = live.join("fullchain.pem");
        let expiry = self
            .read_expiry_unix_ms(&fullchain)
            .await
            .unwrap_or(0);
        let (st, _) = self.classify(expiry);
        self.db
            .ssl_upsert_certificate(
                &primary,
                &primary,
                &spec.domains,
                &live.to_string_lossy(),
                expiry,
                &st,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
        self.db
            .ssl_insert_renewal_event(&primary, "renew", "create: cert obtained")
            .await
            .map_err(|e| e.to_string())?;
        let row = SslCertificateRow {
            primary_domain: primary.clone(),
            cert_name: primary.clone(),
            domains: spec.domains.clone(),
            live_path: live.to_string_lossy().into_owned(),
            expiry_utc_ms: expiry,
            status: st,
            last_error: None,
            updated_at: chrono::Utc::now(),
        };
        let h = resolve_ssl_probe_host(&self.config, &spec.domains, &primary);
        let post_check = Some(
            run_post_check_after_cert(
                &self.config,
                h.as_deref().unwrap_or(""),
            )
            .await,
        );
        let degraded = post_check
            .as_ref()
            .map(|p| {
                p.classified_error == "no_concrete_host"
                    || !p.reload_ok
                    || !p.nginx_test_ok
                    || (!p.upstream_health_ok && p.classified_error != "curl_unavailable")
            })
            .unwrap_or(false);
        let status = if degraded {
            "degraded"
        } else {
            "ok"
        }
        .to_string();
        Ok(SslCreateResponse {
            status,
            cert: Some(self.row_to_proto(&row, true)),
            log_lines,
            post_check,
        })
    }

    pub async fn status(&self, req: SslStatusRequest) -> Result<SslStatusResponse, String> {
        let mut rows = self
            .db
            .ssl_list_certificates()
            .await
            .map_err(|e| e.to_string())?;
        for r in &mut rows {
            let full = Path::new(&r.live_path).join("fullchain.pem");
            if let Some(ex) = Self::read_expiry_unix_ms_sync(&full) {
                r.expiry_utc_ms = ex;
                let (st, _) = self.classify(ex);
                r.status = st;
            }
        }
        let threshold = self.config.expiry_threshold_days;
        let certs: Vec<SslCertRecord> = rows
            .iter()
            .map(|r| self.row_to_proto(r, req.include_paths))
            .collect();
        Ok(SslStatusResponse {
            certs,
            threshold_days: threshold,
        })
    }

    fn glob_match(pattern: &str, domain: &str) -> bool {
        if let Some(suf) = pattern.strip_prefix("*.") {
            return domain == suf || domain.ends_with(&format!(".{suf}"));
        }
        pattern == domain
    }

    fn selector_matches(row: &SslCertificateRow, sel: &SslUpdateSelector) -> Result<bool, String> {
        if !sel.exact_domain.is_empty() {
            return Ok(
                row.primary_domain == sel.exact_domain
                    || row.domains.iter().any(|d| d == &sel.exact_domain),
            );
        }
        if !sel.glob_pattern.is_empty() {
            return Ok(
                Self::glob_match(&sel.glob_pattern, &row.primary_domain)
                    || row
                        .domains
                        .iter()
                        .any(|d| Self::glob_match(&sel.glob_pattern, d)),
            );
        }
        if !sel.regex.is_empty() {
            let re = Regex::new(&sel.regex)
                .map_err(|e| format!("invalid regex: {e}"))?;
            return Ok(
                re.is_match(&row.primary_domain)
                    || row.domains.iter().any(|d| re.is_match(d)),
            );
        }
        Err("set exact_domain, glob_pattern, or regex in selector".into())
    }

    pub async fn update(&self, req: SslUpdateRequest) -> Result<SslUpdateResponse, String> {
        let sel = req
            .selector
            .as_ref()
            .ok_or("missing SslUpdateSelector")?;
        let all = self
            .db
            .ssl_list_certificates()
            .await
            .map_err(|e| e.to_string())?;
        let mut target: Vec<SslCertificateRow> = Vec::new();
        for r in all {
            if Self::selector_matches(&r, sel)? {
                target.push(r);
            }
        }
        if target.is_empty() {
            return Err("no matching certificates in metadata database".into());
        }
        let mut log_lines: Vec<String> = Vec::new();
        let mut updated: Vec<SslCertRecord> = Vec::new();
        let mut probe_host: Option<String> = None;
        for row in &target {
            let is_exp = row.status == "expired"
                || row.expiry_utc_ms
                    < chrono::Utc::now().timestamp_millis();
            let run = if is_exp {
                certbot::force_renew(&self.config, &row.cert_name, req.dry_run).await?
            } else {
                certbot::renew(&self.config, Some(&row.cert_name), req.dry_run).await?
            };
            log_lines.push(format!("[{}] certbot={}", row.primary_domain, run.status));
            if run.status != 0 {
                let _ = self
                    .db
                    .ssl_upsert_certificate(
                        &row.primary_domain,
                        &row.cert_name,
                        &row.domains,
                        &row.live_path,
                        row.expiry_utc_ms,
                        "error",
                        Some("renew failed"),
                    )
                    .await;
                return Err(format!("certbot: {}", run.stderr));
            }
            let full = Path::new(&row.live_path).join("fullchain.pem");
            let expiry = if req.dry_run {
                row.expiry_utc_ms
            } else {
                self.read_expiry_unix_ms(&full).await.unwrap_or(row.expiry_utc_ms)
            };
            let (st, _) = self.classify(expiry);
            self.db
                .ssl_upsert_certificate(
                    &row.primary_domain,
                    &row.cert_name,
                    &row.domains,
                    &row.live_path,
                    expiry,
                    &st,
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            self.db
                .ssl_insert_renewal_event(&row.primary_domain, "renew", "update")
                .await
                .map_err(|e| e.to_string())?;
            if probe_host.is_none() {
                probe_host = resolve_ssl_probe_host(&self.config, &row.domains, &row.primary_domain);
            }
            let mut fresh = row.clone();
            fresh.expiry_utc_ms = expiry;
            fresh.status = st;
            fresh.last_error = None;
            updated.push(self.row_to_proto(&fresh, true));
        }
        let post_check = if req.dry_run {
            None
        } else {
            Some(
                run_post_check_after_cert(
                    &self.config,
                    probe_host.as_deref().unwrap_or(""),
                )
                .await,
            )
        };
        let degraded = post_check
            .as_ref()
            .map(|p| {
                p.classified_error == "no_concrete_host"
                    || !p.reload_ok
                    || !p.nginx_test_ok
                    || (!p.upstream_health_ok && p.classified_error != "curl_unavailable")
            })
            .unwrap_or(false);
        let status = if degraded { "degraded" } else { "ok" }.to_string();
        Ok(SslUpdateResponse {
            status,
            updated,
            log_lines,
            post_check,
        })
    }

    pub async fn check_and_renew(
        &self,
        req: SslCheckAndRenewRequest,
    ) -> Result<SslCheckAndRenewResponse, String> {
        let mut rows = self
            .db
            .ssl_list_certificates()
            .await
            .map_err(|e| e.to_string())?;
        for r in &mut rows {
            let full = Path::new(&r.live_path).join("fullchain.pem");
            if let Some(ex) = Self::read_expiry_unix_ms_sync(&full) {
                r.expiry_utc_ms = ex;
            }
        }
        let th_ms = i64::from(self.config.expiry_threshold_days.max(0)) * 86_400_000;
        let now = chrono::Utc::now().timestamp_millis();
        let mut log_lines: Vec<String> = Vec::new();
        let mut checked: i32 = 0;
        let mut attempted: i32 = 0;
        let mut failed: i32 = 0;
        let mut any_renew_success = false;
        let mut probe_host: Option<String> = None;
        for row in &rows {
            checked = checked.saturating_add(1);
            if !req.force_all && row.expiry_utc_ms > now + th_ms {
                continue;
            }
            attempted = attempted.saturating_add(1);
            let is_exp = row.expiry_utc_ms < now;
            let run = if is_exp {
                certbot::force_renew(&self.config, &row.cert_name, false).await
            } else {
                certbot::renew(&self.config, Some(&row.cert_name), false).await
            }?;
            log_lines.push(format!(
                "[{}] certbot status={}",
                row.primary_domain, run.status
            ));
            if run.status != 0 {
                failed = failed.saturating_add(1);
                let _ = self
                    .db
                    .ssl_upsert_certificate(
                        &row.primary_domain,
                        &row.cert_name,
                        &row.domains,
                        &row.live_path,
                        row.expiry_utc_ms,
                        "error",
                        Some("renew failed"),
                    )
                    .await;
                if self.config.webhook_url.is_some() {
                    self.post_webhook_alert(&row.primary_domain, "ssl_renew_failed")
                        .await;
                }
                let _ = self
                    .db
                    .ssl_insert_renewal_event(
                        &row.primary_domain,
                        "error",
                        "check_and_renew: certbot non-zero",
                    )
                    .await;
                continue;
            }
            any_renew_success = true;
            let full = Path::new(&row.live_path).join("fullchain.pem");
            let expiry = self
                .read_expiry_unix_ms(&full)
                .await
                .unwrap_or(row.expiry_utc_ms);
            let (st, _) = self.classify(expiry);
            self.db
                .ssl_upsert_certificate(
                    &row.primary_domain,
                    &row.cert_name,
                    &row.domains,
                    &row.live_path,
                    expiry,
                    &st,
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            self.db
                .ssl_insert_renewal_event(&row.primary_domain, "renew", "check_and_renew")
                .await
                .map_err(|e| e.to_string())?;
            if probe_host.is_none() {
                probe_host = resolve_ssl_probe_host(&self.config, &row.domains, &row.primary_domain);
            }
        }
        let post_check = if any_renew_success {
            Some(
                run_post_check_after_cert(
                    &self.config,
                    probe_host.as_deref().unwrap_or(""),
                )
                .await,
            )
        } else {
            None
        };
        let degraded = post_check
            .as_ref()
            .map(|p| {
                p.classified_error == "no_concrete_host"
                    || !p.reload_ok
                    || !p.nginx_test_ok
                    || (!p.upstream_health_ok && p.classified_error != "curl_unavailable")
            })
            .unwrap_or(false);
        let status = if degraded {
            "degraded"
        } else {
            "ok"
        }
        .to_string();
        Ok(SslCheckAndRenewResponse {
            status,
            checked,
            attempted_renew: attempted,
            failed,
            log_lines,
            post_check,
        })
    }

    async fn post_webhook_alert(&self, primary_domain: &str, kind: &str) {
        let Some(url) = &self.config.webhook_url else {
            return;
        };
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            let body = serde_json::json!({ "kind": kind, "domain": primary_domain });
            let _ = client.post(url).json(&body).send().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_not_after_millis;

    #[test]
    fn parse_openssl_not_after() {
        let s = "notAfter=Mar  8 12:00:00 2027 GMT\n";
        let ms = parse_not_after_millis(s);
        assert!(ms.is_some());
        assert!(ms.unwrap() > 1_700_000_000_000);
    }
}
