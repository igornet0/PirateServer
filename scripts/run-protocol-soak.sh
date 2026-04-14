#!/usr/bin/env bash
# Long soak loop (merge: test + protocol-bench + protocol-ext). Manual / nightly.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export DOCKER_E2E_CONTROL_API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
export SOAK_DURATION_SEC="${SOAK_DURATION_SEC:-300}"
export SOAK_INTERVAL_SEC="${SOAK_INTERVAL_SEC:-10}"
COMPOSE=(
  docker compose
  -f tests/docker/docker-compose.test.yml
  -f tests/docker/docker-compose.protocol-bench.yml
  -f tests/docker/docker-compose.protocol-ext.yml
)

"${COMPOSE[@]}" up -d --build --quiet-build postgres deploy-server
for _ in $(seq 1 90); do
  if "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json 2>/dev/null; then
    break
  fi
  sleep 1
done
"${COMPOSE[@]}" --profile protocol-bench run --rm bootstrap-grpc-keys
"${COMPOSE[@]}" restart deploy-server
sleep 4
"${COMPOSE[@]}" --profile protocol-soak build -q protocol-soak
"${COMPOSE[@]}" --profile protocol-soak up -d control-api
for _ in $(seq 1 90); do
  if curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok; then
    break
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok
"${COMPOSE[@]}" --profile protocol-soak run --rm protocol-soak

echo "OK: protocol-soak finished"
