# Phase 6+ stack (nginx, PostgreSQL, frontend, control-api)

## Components

| Piece | Role |
|-------|------|
| `deploy-server` | gRPC deploy/rollback; optional `DATABASE_URL` for audit rows. |
| `control-api` | HTTP JSON: `/api/v1/status`, `/releases`, `/history`, `/health`, `/api/v1/nginx/config` (optional). |
| PostgreSQL | Migrations in `server-stack/deploy-db/migrations/`; events + snapshot tables. **Only `deploy-server` runs `sqlx` migrations** on startup when `DATABASE_URL` is set; `control-api` connects only. Start `deploy-server` before `control-api` (see systemd `After=`). |
| `server-stack/frontend/` | Vite + TS static dashboard; `npm run build` → `server-stack/frontend/dist/`. |
| nginx | See `server-stack/deploy/nginx.conf.example` — `[::]:80`, static + `/api/` → control-api. |

## Environment

- `DATABASE_URL` — e.g. `postgresql://user:pass@[::1]:5432/dbname` (IPv6 host must be bracketed).
- `DEPLOY_ROOT` / `--deploy-root` — same path as deploy-server `--root` (for listing `releases/`).
- `GRPC_ENDPOINT` — default `http://[::1]:50051` for control-api → deploy-server.
- `GRPC_SIGNING_KEY_PATH` — optional; path to Ed25519 `identity.json` (same as CLI/desktop after `pair`) so control-api can sign `GetStatus` when deploy-server enforces auth. Omit when using `DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1` on the server.
- `CONTROL_API_PORT` / `--listen-port` — default `8080` (IPv6 all interfaces).

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

Follow the printed commands to run `deploy-server` first, then `control-api`. With `DATABASE_URL` set, **`deploy-server` applies migrations**; `control-api` expects an already-migrated schema.

## Acceptance

1. `deploy-server` running with `--root` and optional `--database-url`.
2. `control-api` returns JSON from `GET /api/v1/status` matching `client status` (via gRPC).
3. `GET /api/v1/releases` lists directories under `releases/`.
4. After a deploy, `GET /api/v1/history` shows rows when PostgreSQL is configured.

See also [LOCAL_GATEWAY.md](LOCAL_GATEWAY.md) for connecting a local PC to the server (VPN, TLS, separate from the deploy CLI).
