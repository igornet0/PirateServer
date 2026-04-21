# server-stack

Server-side deployment and control platform for PirateServer.

## Modules

- [`server`](server/README.md) - core gRPC deploy service.
- [`control-api`](control-api/README.md) - HTTP API for UI and desktop client.
- [`deploy-control`](deploy-control/README.md) - deployment orchestration logic.
- [`deploy-db`](deploy-db/README.md) - persistence and migration layer.
- [`deploy-core`](deploy-core/README.md) - shared deploy domain primitives.
- [`deploy-auth`](deploy-auth/README.md) - auth and signature primitives.
- [`frontend`](frontend/README.md) - browser dashboard.
- [`host-agent`](host-agent/README.md) - host-level management agent.
- [`deploy`](deploy/README.md) - installation scripts and packaging files.

## Documentation

- RU: [`docs/ru/server-client/README.md`](../docs/ru/server-client/README.md)
- EN: [`docs/en/server-client/README.md`](../docs/en/server-client/README.md)
