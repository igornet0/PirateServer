#!/usr/bin/env bash
# Load + latency percentiles + metrics scrape (merge: test + protocol-bench + protocol-ext).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export DOCKER_E2E_CONTROL_API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
export LATENCY_SAMPLES="${LATENCY_SAMPLES:-200}"
export LOAD_RPS_DURATION_SEC="${LOAD_RPS_DURATION_SEC:-5}"
export LOAD_PARALLEL="${LOAD_PARALLEL:-8}"
COMPOSE=(
  docker compose
  -f tests/docker/docker-compose.test.yml
  -f tests/docker/docker-compose.protocol-bench.yml
  -f tests/docker/docker-compose.protocol-ext.yml
)

echo "==> build / start postgres + deploy-server"
"${COMPOSE[@]}" up -d --build --quiet-build postgres deploy-server

echo "==> wait for deploy-server keys"
for _ in $(seq 1 90); do
  if "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json 2>/dev/null; then
    break
  fi
  sleep 1
done
"${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json

echo "==> bootstrap control-api gRPC key"
"${COMPOSE[@]}" --profile protocol-bench run --rm bootstrap-grpc-keys

echo "==> restart deploy-server"
"${COMPOSE[@]}" restart deploy-server
sleep 4

echo "==> build protocol-load + start control-api"
"${COMPOSE[@]}" --profile protocol-load build -q protocol-load
"${COMPOSE[@]}" --profile protocol-load up -d control-api

for _ in $(seq 1 90); do
  if curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok; then
    break
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok

echo "==> run protocol-load"
"${COMPOSE[@]}" --profile protocol-load run --rm protocol-load

echo "OK: protocol-load finished"
