# Docker-based integration tests

The stack under [`docker-compose.test.yml`](../docker-compose.test.yml) mirrors production: **PostgreSQL**, **deploy-server** (gRPC), **control-api** (HTTP), **nginx** (static dashboard + `/api/` proxy).

## Prerequisites

- Docker with Compose v2
- From the repository root

## Run locally

```bash
./scripts/run-docker-e2e.sh
```

This builds images, starts services, runs the `e2e` profile (same checks as CI), and leaves containers running. To stop and remove volumes:

```bash
./scripts/run-docker-e2e.sh --down
```

Manual steps:

```bash
docker compose -f docker-compose.test.yml up -d --build
docker compose -f docker-compose.test.yml --profile e2e run --rm e2e
```

The UI is proxied on **http://localhost:18080** (mapped from nginx `:80`).

## What the e2e script checks

1. `GET /health` on control-api  
2. `GET /api/v1/status` direct and via nginx  
3. `client deploy` of [`tests/fixtures/minimal-app`](../tests/fixtures/minimal-app) (version `v-e2e-1`)  
4. Status `running`, releases list  
5. Second deploy `v-e2e-2`  
6. `client rollback v-e2e-1`  
7. `GET /api/v1/history` contains at least one event  

Nginx **config file** API (`NGINX_CONFIG_PATH` + `nginx -t` / reload) is not exercised in this compose stack: that flow targets a host-level nginx and is documented in [`PHASE6.md`](PHASE6.md).

## Implementation notes

- **Bind address**: `deploy-server` and `control-api` accept `--bind` (default `::`). Compose uses `--bind 0.0.0.0` so containers resolve each other by name over the Docker bridge.  
- **Data directory**: both services share a named volume at `/data`; [`server-stack/deploy/docker/entrypoint-deploy.sh`](../server-stack/deploy/docker/entrypoint-deploy.sh) runs `chown` for user `deploy` (uid 1000) before `exec`.  
- **Images**: [`Dockerfile`](../Dockerfile) targets `runtime-base` (Rust binaries) and `nginx-with-ui` (built frontend + nginx config [`server-stack/deploy/docker/nginx-docker.conf`](../server-stack/deploy/docker/nginx-docker.conf)).

## CI

GitHub Actions workflow: [`.github/workflows/docker-e2e.yml`](../.github/workflows/docker-e2e.yml).

## Future product scope (tracked separately)

The following are **not** covered by this Docker e2e suite and remain separate product or architecture work:

- Multiple isolated **projects** / deploy roots in one control plane  
- **Artifact upload** from the browser (today: `client deploy` CLI over gRPC)  
- **systemd** (or host service) start/stop/restart from the dashboard  
- A dedicated **local PC gateway** agent (see [`LOCAL_GATEWAY.md`](LOCAL_GATEWAY.md))
