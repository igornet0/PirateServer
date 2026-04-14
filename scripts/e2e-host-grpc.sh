#!/usr/bin/env bash
# Optional: from the host, call `client status` against the published gRPC port (requires `client` on PATH).
# Run after: docker compose -f tests/docker/docker-compose.test.yml up -d
set -euo pipefail
HOST="${DOCKER_E2E_HOST:-127.0.0.1}"
PORT="${DOCKER_E2E_GRPC_PORT:-50051}"
ENDPOINT="http://${HOST}:${PORT}"
if ! command -v client >/dev/null 2>&1; then
  echo "skip: no 'client' binary on PATH (build with: cargo build -p deploy-client --release)"
  exit 0
fi
echo "==> host gRPC: client --endpoint $ENDPOINT status"
exec client --endpoint "$ENDPOINT" status
