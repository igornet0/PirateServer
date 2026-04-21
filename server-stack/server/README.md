# server-stack/server

Core gRPC deployment service crate (`deploy-server`).

## Responsibilities

- Accepts pair, status, upload, and deploy RPC calls.
- Coordinates release apply flows with `deploy-control`.
- Collects process/resource state for runtime visibility.

## Related docs

- RU: [`docs/ru/server-client/README.md`](../../docs/ru/server-client/README.md)
- EN: [`docs/en/server-client/README.md`](../../docs/en/server-client/README.md)
