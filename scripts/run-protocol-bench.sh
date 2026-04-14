#!/usr/bin/env bash
# From repo root: build stack, bootstrap control-api gRPC identity, restart deploy-server to load peers,
# then run protocol throughput + security bench (see scripts/protocol-bench-docker.sh).
#
# Usage:
#   ./scripts/run-protocol-bench.sh
# Optional:
#   BENCH_BYTES=67108864 BENCH_RUNS=5 ./scripts/run-protocol-bench.sh
# Teardown:
#   docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.protocol-bench.yml --profile protocol-bench down -v
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export DOCKER_E2E_CONTROL_API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
export BENCH_BYTES="${BENCH_BYTES:-33554432}"
export BENCH_RUNS="${BENCH_RUNS:-3}"
COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.protocol-bench.yml)

echo "==> build / start postgres + deploy-server"
"${COMPOSE[@]}" up -d --build --quiet-build postgres deploy-server

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
"${COMPOSE[@]}" --profile protocol-bench run --rm bootstrap-grpc-keys

echo "==> restart deploy-server so it reloads authorized_peers.json"
"${COMPOSE[@]}" restart deploy-server
sleep 4

echo "==> build bench runner image (grpcurl + client) + start upstreams"
"${COMPOSE[@]}" --profile protocol-bench build -q protocol-bench
"${COMPOSE[@]}" --profile protocol-bench up -d control-api bench-upstream deny-upstream

echo "==> wait for control-api health (host port ${DOCKER_E2E_CONTROL_API_PORT})"
for _ in $(seq 1 90); do
  if curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok; then
    break
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok

echo "==> run protocol bench container"
"${COMPOSE[@]}" --profile protocol-bench run --rm protocol-bench

echo "OK: protocol bench finished"
