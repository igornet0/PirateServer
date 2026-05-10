# Nginx universal API (control-api)

Endpoints (JWT `Authorization: Bearer`):

- `GET /api/v1/nginx/sites` — inventory: main `nginx.conf`, `conf.d/*.conf`, `sites-available` + `sites-enabled` symlinks, per-file domains / SSL heuristics, `managed_by: pirate|external` (Pirate marker or path), global duplicate-`server_name` conflicts across **enabled** vhosts.
- `POST /api/v1/nginx/preflight` — body `NginxPreflightProposed` (optional `action`, `path`). Returns full inventory plus `blockers` (e.g. missing path for proposed action).
- `POST /api/v1/nginx/action` — body `NginxActionBody`:
  - `enable_site` — `available_path` (symlink via `sudo -n pirate-nginx-ops.sh enable-site …`, not bare `sudo ln`)
  - `disable_site` — `enabled_path` or `path` (via `pirate-nginx-ops.sh disable-site`)
  - `set_server_name` — `path`, `server_name` (first `server { }` block)
  - `set_ssl` — `path`, `ssl_enabled`, optional `ssl_cert_path` / `ssl_key_path` (defaults to Let’s Encrypt paths from first `server_name` when omitted). Optional `issue_certificate_if_missing: true` runs `certbot certonly` when those default paths are missing (uses host env `SSL_MODE`, `SSL_EMAIL`, `SSL_WEBROOT`, `SSL_USE_SUDO`, `SSL_CERTBOT_BIN`; optional body `acme_email`, `acme_staging`, `acme_dry_run`). ACME domain: `post_check_host` if set, else first `server_name`. `SSL_MODE=dns` is rejected here (use gRPC `SslCreate`).
  - `validate` — `nginx -t` (via `sudo -n pirate-nginx-ops.sh validate` as root)
  - `reload` — `systemctl reload nginx` (via `pirate-nginx-ops.sh reload`)

Vhost writes under `sites-available/` use `pirate-nginx-apply-site.sh`. All other privileged checks and writes (inventory `nginx -t`, `PUT /api/v1/nginx/config`, non–sites-available paths in `set_server_name` / `set_ssl`) use `pirate-nginx-ops.sh` (`validate`, `reload`, `apply-config`). Install ships the script and `99-pirate-smb` must list it for NOPASSWD.

**Compatibility:** older control-api without these routes: desktop shows a hint to upgrade; existing `/api/v1/nginx/status|site|ensure` unchanged.
