#!/usr/bin/env bash
# From repo root: build stack, bootstrap control-api gRPC identity, restart deploy-server to load peers,
# then run VLESS / Trojan / VMess tests via client `board` (see scripts/wire-tunnel-e2e.sh).
#
# Requires Docker Compose v2 (restart + run).
#
# Usage:
#   ./scripts/run-wire-tunnel-e2e.sh
# Teardown:
#   docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.wire-e2e.yml --profile wire-e2e down -v
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export DOCKER_E2E_CONTROL_API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.wire-e2e.yml)

echo "==> build / start postgres + deploy-server"
"${COMPOSE[@]}" up -d --build postgres deploy-server

echo "==> wait for deploy-server keys"
for _ in $(seq 1 90); do
  if "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json 2>/dev/null; then
    break
  fi
  sleep 1
done
if ! "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json; then
  echo "timeout: /data/.keys/server_ed25519.json missing" >&2
  exit 1
fi

echo "==> bootstrap control-api gRPC key (writes /data/.keys/control_api_ed25519.json)"
"${COMPOSE[@]}" --profile wire-e2e run --rm bootstrap-grpc-keys

echo "==> restart deploy-server so it reloads authorized_peers.json (control-api must be allowed to sign CreateConnection)"
"${COMPOSE[@]}" restart deploy-server
sleep 4

echo "==> start control-api, nginx, wire-upstream"
"${COMPOSE[@]}" --profile wire-e2e up -d control-api nginx wire-upstream

echo "==> wait for control-api health (host port ${DOCKER_E2E_CONTROL_API_PORT})"
for _ in $(seq 1 90); do
  if curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok; then
    break
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok

echo "==> run wire tunnel e2e (VLESS, Trojan, VMess)"
"${COMPOSE[@]}" --profile wire-e2e run --rm wire-e2e

echo "OK: wire tunnel e2e passed"
