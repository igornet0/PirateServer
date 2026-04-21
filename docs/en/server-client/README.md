# Server Client (EN)

The server stack receives deployment requests from local clients, persists metadata, applies releases, and exposes runtime control via API/UI.

## Modules

- [`server-stack/server`](../../../server-stack/server/README.md) - core gRPC deployment service.
- [`server-stack/control-api`](../../../server-stack/control-api/README.md) - HTTP API for dashboard and desktop client.
- [`server-stack/deploy-control`](../../../server-stack/deploy-control/README.md) - deploy orchestration across files, nginx, and DB.
- [`server-stack/deploy-db`](../../../server-stack/deploy-db/README.md) - database layer and migrations.
- [`server-stack/frontend`](../../../server-stack/frontend/README.md) - web dashboard client.
- [`server-stack/deploy`](../../../server-stack/deploy/README.md) - bare-metal install scripts and environment setup.

## Primary Flow

1. Local client pairs and uploads artifacts.
2. `server` and `deploy-control` validate and apply the release.
3. State and resource tracking are available via `control-api` and `frontend`.
