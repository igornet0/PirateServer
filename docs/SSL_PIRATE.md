# SSL / Certbot via `pirate ssl`

## Overview

- **Server:** `deploy-server` exposes gRPC methods `SslCreate`, `SslStatus`, `SslUpdate`, `SslCheckAndRenew` (see `proto/deploy.proto`).
- **Storage:** Certificate **metadata** (domains, expiry, paths under `/etc/letsencrypt/live/...`) is stored in the deploy metadata DB (`ssl_certificates`, `ssl_renewal_events`). **Private keys are never stored in the DB.**
- **Tooling:** The host must have `certbot` and `openssl` on `PATH`. Certbot usually requires **passwordless `sudo`** for the user running `deploy-server` (see `99-pirate-smb`: `/usr/bin/certbot` must match `SSL_CERTBOT_BIN`), or set `SSL_USE_SUDO=0` when the process can run certbot without elevation.
- **Nginx reload after renew:** When `SSL_USE_SUDO=1`, reload uses `sudo -n /usr/local/lib/pirate/pirate-nginx-ops.sh reload` (override with `PIRATE_NGINX_OPS_SCRIPT`). The same script is used by control-api for privileged `nginx -t` / reload.
- **Scheduler:** A background task in `deploy-server` calls the same renew logic as `pirate ssl check-and-renew` on `SSL_CHECK_INTERVAL` (default 86400s). Set `SSL_ENABLE_SCHEDULER=0` to disable.

## Client usage

```bash
# After `pirate auth` to the same gRPC endpoint:
pirate ssl create -d example.com -d www.example.com --dry-run
pirate ssl status
pirate ssl status -v
pirate ssl update -u api.example.com
pirate ssl update --ur '*.example.com'
pirate ssl check-and-renew
pirate ssl check-and-renew --force-all
```

Global `--url` / `--endpoint` selects the deploy-server gRPC address, as for other `pirate` commands.

## Server environment

See `server-stack/deploy/ubuntu/env.example` (section **gRPC `pirate ssl` + Certbot**). Key variables:

| Variable | Purpose |
|----------|---------|
| `SSL_EMAIL` | ACME registration email (required for real issuance) |
| `SSL_MODE` | `nginx`, `standalone`, `webroot`, or `dns` |
| `SSL_WEBROOT` | Webroot path when using `webroot` |
| `SSL_CHECK_INTERVAL` | Scheduler period (seconds) |
| `SSL_EXPIRY_THRESHOLD_DAYS` | Renew window (default 7) |
| `SSL_CERTBOT_DNS_PLUGIN` / `SSL_CERTBOT_DNS_CREDENTIALS` | Cloudflare DNS-01 (v1 wiring) |

## Nginx preflight (`control-api`)

- The dashboard’s **Check conflicts / preflight** for nginx reads vhost files and checks that each `ssl_certificate` path exists.
- Paths are **not** lowercased before the check (case-sensitive filesystems are respected).
- Under typical permissions, only `root` can `stat` files under `/etc/letsencrypt/live/...`. The preflight uses the same logic as certbot helpers: `Path::is_file` first, then optional **passwordless** `sudo -n test -f <path>` when `SSL_USE_SUDO=1` and the process is not root (see `deploy-control` `privileged_path_is_file`).
- If `ssl_certificate` uses a **variable** (`$…`) or a **non-absolute** path, preflight reports that it **did not verify** file presence (not a false “missing” for a valid Let’s Encrypt path).
- If `control-api` runs in a **container** without the host’s `/etc/letsencrypt` mounted, preflight may report missing certs even though they exist on the host; mount the same paths or run the API on the host.

## Rollout stages

1. Deploy `deploy-server` with new binary + DB migrations (automatic on start when `DEPLOY_SQLITE_URL` is set).
2. Configure `SSL_*` env and sudoers for certbot and `pirate-nginx-ops.sh` (OTA/install updates `99-pirate-smb`).
3. Verify with `pirate ssl status` (empty list until first `create`).
4. Run `pirate ssl create ...` (staging with `--staging` / server `SSL` + certbot `--test-cert` via client flag).
5. Enable scheduler once happy; monitor logs and optional `SSL_ALERT_WEBHOOK_URL`.
