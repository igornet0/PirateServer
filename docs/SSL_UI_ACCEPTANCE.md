# SSL + UI (nginx) — acceptance and e2e checks

## Preconditions

- `curl` on the deploy host (for HTTPS post-check; without it, response may be `degraded` with `classified_error=curl_unavailable`).
- `sudo` NOPASSWD for `pirate-nginx-ops.sh` when `SSL_USE_SUDO=1` (as for control-api / deploy-server).
- UI domain resolves and nginx vhost `server_name` matches the certificate; for wildcard-only certs, set a concrete `post_check_host` on `set_ssl` or add a non-wildcard name for probing.

## Happy path (gRPC + desktop UI)

1. **Issue cert** (dry-run + staging, then real): SslCreate returns `status=ok` and `post_check` with `nginx_test_ok`, `reload_ok`, `upstream_health_ok` true.
2. **Nginx set_ssl** (control-api `POST /api/v1/nginx/action` with `action=set_ssl`, `ssl_enabled=true`): response `ok: true` and `post_check` summary "HTTPS check passed" (or `curl` skip message).
3. **One-shot SSL on**: same request with `issue_certificate_if_missing: true` (desktop sends this when enabling SSL) runs `certbot certonly` if default Let’s Encrypt `fullchain.pem` is missing; requires `SSL_EMAIL` (or `acme_email`), `SSL_MODE` (nginx/webroot/standalone), NOPASSWD `certbot` for the control-api user. Not supported for `SSL_MODE=dns` (use gRPC `SslCreate`).
4. **Degraded but safe**: if HTTPS returns 5xx, `set_ssl` response `ok: false` and `post_check.rollback_performed: true` when restore succeeded; previous vhost restored.
5. **Scheduler renew**: at least one successful renew yields `check_and_renew` with optional `post_check` and `status=ok` when nginx + probe pass.

## Failure scenarios to verify

- **502 through nginx**: `upstream_health_ok: false`, `classified_error: upstream_5xx`, gRPC `status: degraded` (cert may still be valid in DB).
- **Reload failed** with `SSL_STRICT_NGINX_RELOAD=1`: gRPC `degraded`, `post_check.reload_ok: false`.
- **TLS hostname / SAN mismatch** (`curl` exit 60): `post_check.classified=tls_name_mismatch`; `set_ssl` does **not** auto-rollback (fix cert, `server_name`, or `post_check_host`). Preflight may fail earlier with `openssl x509 -checkhost`.
- **Wrong vhost** after enable: probe fails with `upstream_5xx` / connection classes → `set_ssl` rollback when policy applies.

## Automation ideas

- In CI, mock `curl` and nginx ops scripts; unit-test `https_probe_localhost_resolve` and `set_ssl` rollback path.
- Staging host: `SSL_POST_CHECK_PATH=/health` (or the UI’s real path) to avoid false 404 on `/`.
