//! SSL / ACME process configuration from environment.

/// Resolved SSL settings for Certbot and scheduler.
#[derive(Debug, Clone)]
pub struct SslProcessConfig {
    pub provider: String,
    pub email: Option<String>,
    pub mode: SslModeResolved,
    pub webroot_path: Option<String>,
    pub check_interval_secs: u64,
    pub expiry_threshold_days: i32,
    pub certbot_bin: String,
    /// Optional extra args (space-safe single token; prefer SSL_CERTBOT_EXTRA_ARGS split by shlex in future).
    pub certbot_extra_args: Vec<String>,
    pub dns_plugin: Option<String>,
    /// Path to INI for `certbot --dns-*` (no secrets in logs from this string — file path only).
    pub dns_credentials: Option<String>,
    pub reload_cmd: Option<String>,
    pub webhook_url: Option<String>,
    /// If true, `certbot` may need passwordless `sudo` on the host.
    pub use_sudo: bool,
    /// When true, `SslCreate` / renew paths run `nginx -t` + optional HTTPS smoke probe after reload.
    pub post_check_enabled: bool,
    /// Path (GET) for local HTTPS smoke probe, e.g. `/` or `/health`.
    pub post_check_path: String,
    pub post_check_port: u16,
    /// Loopback for `curl --resolve` (default 127.0.0.1).
    pub post_check_loopback: String,
    /// Optional SNI hostname for HTTPS smoke probe (overrides primary/first domain).
    pub post_check_host: String,
    /// When true, a failed `nginx` reload is reflected in gRPC as `degraded` with `post_check`.
    pub strict_nginx_reload: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslModeResolved {
    Nginx,
    Standalone,
    Webroot,
    Dns,
}

impl SslModeResolved {
    pub fn from_env_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nginx" => Some(Self::Nginx),
            "standalone" => Some(Self::Standalone),
            "webroot" => Some(Self::Webroot),
            "dns" => Some(Self::Dns),
            _ => None,
        }
    }
}

fn parse_u64(s: &str, default: u64) -> u64 {
    s.trim().parse::<u64>().unwrap_or(default)
}

fn parse_i32(s: &str, default: i32) -> i32 {
    s.trim().parse::<i32>().unwrap_or(default)
}

/// Split `SSL_CERTBOT_EXTRA_ARGS` into tokens (very naive: split on spaces; quoted strings not supported).
fn extra_args_from_env() -> Vec<String> {
    std::env::var("SSL_CERTBOT_EXTRA_ARGS")
        .ok()
        .map(|s| {
            s.split_whitespace()
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Load from env. `email` is required for `certonly` in most cases — caller validates before create.
pub fn load_ssl_config() -> SslProcessConfig {
    let mode = std::env::var("SSL_MODE")
        .ok()
        .as_deref()
        .and_then(SslModeResolved::from_env_str)
        .unwrap_or(SslModeResolved::Nginx);
    SslProcessConfig {
        provider: std::env::var("SSL_PROVIDER")
            .unwrap_or_else(|_| "certbot".to_string()),
        email: std::env::var("SSL_EMAIL").ok().filter(|s| !s.trim().is_empty()),
        mode,
        webroot_path: std::env::var("SSL_WEBROOT").ok().filter(|s| !s.trim().is_empty()),
        check_interval_secs: std::env::var("SSL_CHECK_INTERVAL")
            .as_deref()
            .map(|s| parse_u64(s, 86_400))
            .unwrap_or(86_400),
        expiry_threshold_days: std::env::var("SSL_EXPIRY_THRESHOLD_DAYS")
            .as_deref()
            .map(|s| parse_i32(s, 7))
            .unwrap_or(7)
            .max(0),
        certbot_bin: std::env::var("SSL_CERTBOT_BIN")
            .unwrap_or_else(|_| "certbot".to_string()),
        certbot_extra_args: extra_args_from_env(),
        dns_plugin: std::env::var("SSL_CERTBOT_DNS_PLUGIN")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        dns_credentials: std::env::var("SSL_CERTBOT_DNS_CREDENTIALS")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        reload_cmd: std::env::var("SSL_RELOAD_CMD")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        webhook_url: std::env::var("SSL_ALERT_WEBHOOK_URL")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        use_sudo: std::env::var("SSL_USE_SUDO")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true),
        post_check_enabled: std::env::var("SSL_POST_CHECK_ENABLED")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true),
        post_check_path: std::env::var("SSL_POST_CHECK_PATH")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/".to_string()),
        post_check_port: std::env::var("SSL_POST_CHECK_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(443),
        post_check_loopback: std::env::var("SSL_POST_CHECK_LOOPBACK")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        post_check_host: std::env::var("SSL_POST_CHECK_HOST")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default(),
        strict_nginx_reload: std::env::var("SSL_STRICT_NGINX_RELOAD")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval() {
        assert!(load_ssl_config().check_interval_secs >= 60);
    }
}
