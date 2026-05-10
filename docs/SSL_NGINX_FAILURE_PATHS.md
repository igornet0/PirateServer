# SSL → Nginx → UI: where 502 can appear and false success

## Request path

1. **Certbot** issues/reloads certs under `/etc/letsencrypt/live/<name>/`.
2. **Deploy-server SSL** (`server-stack/server/src/ssl/service.rs`) updates DB, runs **nginx reload** (ops script or `SSL_RELOAD_CMD`).
3. **Nginx vhost** must reference cert paths and `proxy_pass` to a **live** UI/control upstream. Control-plane edits use `pirate-nginx-ops.sh` and `server-stack/deploy-control/src/nginx_universal.rs` (`set_ssl`, `enable_site`, etc.).
4. **Browser/health check** goes `HTTPS:443` → `nginx` → `upstream` (e.g. `127.0.0.1:8080`).

## When “SSL OK” but UI shows 502 (false success)

- **Reload failed** (logged only): cert on disk, nginx not reloaded; old vhost.
- **`nginx -t` ok, upstream down / wrong port**: certbot and reload succeed, runtime request returns 502.
- **Wrong vhost** after `set_ssl` (e.g. first `server` block, duplicate `server_name`): TLS may work on another vhost, UI hits wrong `proxy_pass`.
- **Certificate vs nginx mismatch**: cert issued, `ssl_certificate` not updated in the site the domain uses.
- **Wildcard / DNS-01 only**: no HTTP-01 to validate UI; UI probe must use a **concrete host** or be skipped (documented in env).

## Mitigations in code

- **Post-check** after SSL operations (probe `https` with `--resolve` to loopback) and **structured `post_check` in gRPC** responses.
- **`set_ssl` backup + rollback** in `apply_nginx_universal_action` if the HTTPS probe returns 5xx or connection failure (when post-check is enabled).
- **Desktop UI** surfaces `post_check` so a green cert row does not imply a green end-to-end path.

## Env knobs (see `server-stack/deploy/ubuntu/env.example`)

- `SSL_POST_CHECK_ENABLED`, `SSL_POST_CHECK_PATH`, `SSL_POST_CHECK_PORT`, `SSL_POST_CHECK_LOOPBACK`, `SSL_STRICT_NGINX_RELOAD`.
