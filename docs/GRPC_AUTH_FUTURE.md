# gRPC authentication (deploy-server)

## Implemented: Ed25519 pairing and signed requests

[`proto/deploy.proto`](../proto/deploy.proto) includes `Pair` plus existing `Upload` / `GetStatus` / `Rollback`.

- **Server** ([`deploy-server`](../server-stack/server/)): keys under `<root>/.keys` (or `DEPLOY_KEYS_DIR`): `server_ed25519.json`, `authorized_peers.json`, `pairing.code`. On startup (when auth is enabled) logs a JSON **install bundle**: `token` (server public key, URL-safe Base64), `url` (reachable gRPC HTTP/2 URL), `pairing` (enrollment secret).
- **Clients** ([`deploy-client`](../local-stack/client/), [desktop](../local-stack/desktop-client/)): Ed25519 identity at config path; after `Pair`, all RPCs send metadata (`x-deploy-pubkey`, `x-deploy-ts`, `x-deploy-nonce`, `x-deploy-sig`, and for uploads `x-deploy-version`).
- **control-api** ([`control-api`](../server-stack/control-api/)): optional `GRPC_SIGNING_KEY_PATH` to a client-format `identity.json` that was registered via `Pair`, so `GetStatus` works when the server enforces auth.

## Dev / e2e: open gRPC

Set `DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1` (or `--allow-unauthenticated`) so `deploy-server` skips verification. The Docker test stack ([`docker-compose.test.yml`](../docker-compose.test.yml)) sets this for CI.

## TLS

Signing does **not** encrypt traffic. For internet-facing endpoints, add TLS (or mTLS) in front of gRPC separately.

## See also

- [`PHASE6.md`](PHASE6.md) — control-api env vars including `GRPC_SIGNING_KEY_PATH`.
