# Phase 6+ stack (nginx, metadata DB, frontend, control-api)

## Components

| Piece | Role |
|-------|------|
| `deploy-server` | gRPC deploy/rollback; optional metadata URL (`DEPLOY_SQLITE_URL` or `DATABASE_URL`) for audit rows. |
| `control-api` | HTTP JSON: `/api/v1/status`, `/releases`, `/history`, `/health`, `/api/v1/nginx/config` (optional). |
| Metadata | **Native install (`install.sh`):** SQLite file via `DEPLOY_SQLITE_URL`. **Docker / CI:** PostgreSQL via `DATABASE_URL`. Migrations live in `server-stack/deploy-db/migrations/` (Postgres) and `migrations_sqlite/` (SQLite). **Only `deploy-server` runs `sqlx` migrations** on startup when a metadata URL is set; `control-api` connects only. Start `deploy-server` before `control-api` (see systemd `After=`). |
| PostgreSQL explorer | Optional dashboard schema browser: same pool as metadata when metadata is PostgreSQL, or separate **`POSTGRES_EXPLORER_URL`** when metadata is SQLite-only. |
| `server-stack/frontend/` | Vite + TS static dashboard; `npm run build` → `server-stack/frontend/dist/`. |
| nginx | See `server-stack/deploy/nginx.conf.example` — `[::]:80`, static + `/api/` → control-api. |

## Environment

### Native install: Linux user `pirate`

On Ubuntu, [`install.sh`](../server-stack/deploy/ubuntu/install.sh) creates a **`pirate`** account (home `/var/lib/pirate`, shell `/bin/bash`), adds it to the **`sudo`** group for interactive administration, and runs **`deploy-server`** and **`control-api`** as **`User=pirate`** in systemd. **`deploy-server` still refuses to run as root** (by design). The env file `/etc/pirate-deploy.env` is typically **`chown root:pirate`** so the service user can read secrets.

SMB mounts from the dashboard use **`sudo`** to run fixed helper scripts under **`/usr/local/lib/pirate/`**; **`/etc/sudoers.d/99-pirate-smb`** grants **`NOPASSWD`** only for `pirate-smb-mount.sh` and `pirate-smb-umount.sh` (non-interactive `control-api` cannot prompt for a password). Using one OS account for both daemons and `sudo` is convenient but increases impact if the process is compromised; restrict network exposure accordingly.

- **`DEPLOY_SQLITE_URL`** — native install, e.g. `sqlite:///var/lib/pirate/deploy/deploy.db`. Takes precedence over `DATABASE_URL` when both are set.
- **`DATABASE_URL`** — PostgreSQL metadata URL, e.g. `postgresql://user:pass@[::1]:5432/dbname` (IPv6 host must be bracketed). Used in Docker Compose for the full stack.
- **`POSTGRES_EXPLORER_URL`** — optional separate PostgreSQL for the built-in explorer UI when metadata is stored in SQLite only.

### Optional PostgreSQL on Ubuntu (native install)

The Linux bundle includes **`lib/pirate/`** (installed to **`/usr/local/lib/pirate/`**): `install-postgresql.sh`, SMB helpers, and other **`install-*.sh`** scripts. Run **`install-postgresql.sh`** with **sudo** as root, or set **`pirate_INSTALL_POSTGRESQL=1`** when running **`install.sh`**, to install packages, create the explorer DB user, and obtain **`POSTGRES_EXPLORER_URL=`**. **`install.sh`** appends that line to **`/etc/pirate-deploy.env`** when the flag is set; otherwise add it manually, then **`systemctl restart control-api`** (and ensure **`deploy-server`** has started once so metadata migrations apply). The dashboard can also store a **`postgresql`** data source (saved credentials); that is separate from **`POSTGRES_EXPLORER_URL`**, which enables the built-in schema browser.

Optional **`pirate_INSTALL_*`** flags for other databases on the host: **`pirate_INSTALL_MYSQL`**, **`pirate_INSTALL_REDIS`**, **`pirate_INSTALL_MONGODB`**, **`pirate_INSTALL_MSSQL`**, **`pirate_INSTALL_CLICKHOUSE`**, **`pirate_INSTALL_ORACLE_NOTES`**, and **`pirate_INSTALL_CIFS=1`** for **`cifs-utils`** (SMB).
- `DEPLOY_ROOT` / `--deploy-root` — same path as deploy-server `--root` (for listing `releases/`).
- `GRPC_ENDPOINT` — default `http://[::1]:50051` for control-api → deploy-server.
- `GRPC_SIGNING_KEY_PATH` — optional; path to Ed25519 `identity.json` (same as CLI/desktop after `pair`) so control-api can sign `GetStatus` when deploy-server enforces auth. Omit when using `DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1` on the server.
- `CONTROL_API_PORT` / `--listen-port` — default `8080`.
- `CONTROL_API_BIND` — адрес прослушивания (`control-api`): в [`install.sh`](../server-stack/deploy/ubuntu/install.sh) для нативной установки задаётся **`127.0.0.1`** (только localhost); без переменной по умолчанию **`::`** (все интерфейсы; см. Docker).

### Security and CORS (control-api)

- **`CONTROL_API_BEARER_TOKEN`** — if set, every `/api/v1/*` route requires header `Authorization: Bearer <token>`. `/health` stays unauthenticated. Use in production when the API is reachable without nginx auth.
- **CORS**
  - **`CONTROL_API_CORS_ALLOW_ANY=1`** — permissive CORS (same as legacy `CorsLayer::permissive()`). Suitable for local dev with Vite (`npm run dev`) calling `control-api` on another origin.
  - Otherwise **`CONTROL_API_CORS_ORIGINS`** — comma-separated allowed origins (e.g. `http://localhost:5173,https://dashboard.example.com`). If unset and `ALLOW_ANY` is off, a restrictive CORS layer is used (same-origin browser traffic to nginx does not need CORS).

### JSON errors (`/api/v1/*`)

Failed requests return JSON of the form:

```json
{ "error": { "code": "bad_gateway", "message": "..." } }
```

Codes include `bad_gateway`, `internal`, `unauthorized`, `service_unavailable` (HTTP status matches the situation).

### Nginx config via API / UI

When `NGINX_CONFIG_PATH` is set on **control-api**, the dashboard can load and save that file:

- `GET /api/v1/nginx/config` — JSON `{ path, content, enabled }` (`enabled: false` if path not configured).
- `PUT /api/v1/nginx/config` — JSON `{ "content": "..." }`. Writes the file, runs `nginx -t` (or `nginx -t -c <path>` if `NGINX_TEST_FULL_CONFIG=true`), then `nginx -s reload`. On `nginx -t` failure the previous file content is restored.

Optional:

- `NGINX_TEST_FULL_CONFIG` — default `false`: validate with `nginx -t` (default main config). Set `true` to validate the edited file as a **full** config: `nginx -t -c $NGINX_CONFIG_PATH`.
- `NGINX_ADMIN_TOKEN` — if set, `PUT` requires nginx admin credentials: header `X-Nginx-Admin-Token: <token>` (use when `CONTROL_API_BEARER_TOKEN` already occupies `Authorization`), or legacy `Authorization: Bearer <nginx-token>`.

The `control-api` process must be allowed to write `NGINX_CONFIG_PATH` and to run `nginx -t` / `nginx -s reload` (often requires matching the user that owns nginx or `sudo` — document for your OS).

## IPv6

- deploy-server listens on `[::]:50051`.
- control-api listens on `[::]:8080`.
- Clients use `http://[::1]:...` on loopback.

## Quick start

```bash
./scripts/bootstrap-phase6.sh
```

Follow the printed commands to run `deploy-server` first, then `control-api`. With a metadata URL set, **`deploy-server` applies migrations**; `control-api` expects an already-migrated schema.

## Acceptance

1. `deploy-server` running with `--root` and optional `--deploy-sqlite-url` / `--database-url`.
2. `control-api` returns JSON from `GET /api/v1/status` matching `client status` (via gRPC).
3. `GET /api/v1/releases` lists directories under `releases/`.
4. After a deploy, `GET /api/v1/history` shows rows when a metadata database is configured.

See also [LOCAL_GATEWAY.md](LOCAL_GATEWAY.md) for connecting a local PC to the server (VPN, TLS, separate from the deploy CLI).
