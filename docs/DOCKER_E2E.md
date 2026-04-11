# Docker-based integration tests

The stack under [`docker-compose.test.yml`](../docker-compose.test.yml) mirrors production: **PostgreSQL**, **deploy-server** (gRPC), **control-api** (HTTP), **nginx** (static dashboard + `/api/` proxy).

## Prerequisites

- Docker with Compose v2
- From the repository root

## Connection bundle (host)

After `docker compose … up`, the stack prints a **connection bundle** (also available via `./scripts/print-docker-connection.sh`):

- **gRPC** — published as `localhost:${DOCKER_E2E_GRPC_PORT:-50051}` (default `50051`) so a local [`client`](../local-stack/client/) binary can use `client --endpoint http://127.0.0.1:50051 …`.
- **Dashboard** — `http://localhost:18080` (override with `DOCKER_E2E_DASHBOARD_PORT`).
- **control-api** — direct HTTP on `localhost:${DOCKER_E2E_CONTROL_API_PORT:-18081}` → container `8080`.

`./scripts/run-docker-e2e.sh` runs `print-docker-connection.sh` automatically after `up`.

gRPC has **no** application-level token in this repo; see [`GRPC_AUTH_FUTURE.md`](GRPC_AUTH_FUTURE.md). HTTP `/api/v1/*` can optionally require a Bearer token (see below).

## Run locally

### Default (core flows)

```bash
./scripts/run-docker-e2e.sh
```

Tear down volumes after:

```bash
./scripts/run-docker-e2e.sh --down
```

Manual steps:

```bash
docker compose -f docker-compose.test.yml up -d --build
bash scripts/print-docker-connection.sh
docker compose -f docker-compose.test.yml --profile e2e run --rm e2e
```

The UI is proxied on **http://localhost:18080** (mapped from nginx `:80`).

### control-api Bearer token

Merge [`docker-compose.bearer-override.yml`](../docker-compose.bearer-override.yml) and set `DOCKER_E2E_API_TOKEN`:

```bash
export DOCKER_E2E_API_TOKEN=your-secret
./scripts/run-docker-e2e-bearer.sh --down
```

### Nginx config API (`GET`/`PUT /api/v1/nginx/config`)

Uses [`docker-compose.nginx-e2e.yml`](../docker-compose.nginx-e2e.yml) (control-api image with nginx + [`entrypoint-nginx-e2e.sh`](../server-stack/deploy/docker/entrypoint-nginx-e2e.sh)):

```bash
./scripts/run-docker-e2e-nginx.sh --down
```

### Optional: host `client` against published gRPC

With the stack up:

```bash
./scripts/e2e-host-grpc.sh
```

Skips if `client` is not on `PATH` (build with `cargo build -p deploy-client --release` and ensure the binary is named `client`).

## What the e2e script checks

[`scripts/e2e-docker.sh`](../scripts/e2e-docker.sh):

1. If `DOCKER_E2E_API_TOKEN` is set: `GET /api/v1/status` without `Authorization` returns **401**.
2. `GET /health` on control-api (no Bearer).
3. `GET /api/v1/status` direct and via nginx (Bearer when token is set).
4. `client deploy` of [`tests/fixtures/minimal-app`](../tests/fixtures/minimal-app) (`v-e2e-1`, then `v-e2e-2`).
5. Status, releases, `client rollback`, history.
6. If `NGINX_E2E_TESTS=1`: `GET`/`PUT /api/v1/nginx/config` with [`tests/fixtures/nginx-e2e-put.conf`](../tests/fixtures/nginx-e2e-put.conf).

## CI

GitHub Actions: [`.github/workflows/docker-e2e.yml`](../.github/workflows/docker-e2e.yml) runs the default stack, bearer override, and nginx merge flows sequentially.

## Desktop client (future)

The Tauri [`pirate-desktop`](../local-stack/desktop-client/) crate does not yet call `DeployService` over gRPC. When it does, add a CI step (or a new script) that builds `client` / desktop and targets `GRPC_ENDPOINT` from the connection bundle above.

## Future product scope (tracked separately)

The following are **not** fully covered by automation alone:

- Multiple isolated **projects** / deploy roots in one control plane  
- **Artifact upload** from the browser (today: `client deploy` CLI over gRPC)  
- **systemd** (or host service) start/stop/restart from the dashboard  
- A dedicated **local PC gateway** agent (see [`LOCAL_GATEWAY.md`](LOCAL_GATEWAY.md))

## Troubleshooting

- **PostgreSQL container exits immediately** (`Exited (1)`): run `docker compose -f docker-compose.test.yml logs postgres`. If you see `No space left on device` during `initdb`, free disk space on the host or increase the Docker Desktop disk image limit (**Settings → Resources**), then `docker system prune` as needed. After changing the Postgres image or fixing a corrupted volume, run `docker compose -f docker-compose.test.yml down -v` and `up` again (see `make -f Makefile.docker doctor`).

## Implementation notes

- **Bind address**: `deploy-server` and `control-api` accept `--bind` (default `::`). Compose uses `--bind 0.0.0.0` so containers resolve each other by name over the Docker bridge.  
- **Data directory**: both services share a named volume at `/data`; [`server-stack/deploy/docker/entrypoint-deploy.sh`](../server-stack/deploy/docker/entrypoint-deploy.sh) runs `chown` for user `deploy` (uid 1000) before `exec`.  
- **Images**: [`Dockerfile`](../Dockerfile) targets `runtime-base` (Rust binaries + `jq` for e2e JSON), `nginx-with-ui` (built frontend + nginx config [`server-stack/deploy/docker/nginx-docker.conf`](../server-stack/deploy/docker/nginx-docker.conf)), and `runtime-nginx-e2e` for nginx API tests.
