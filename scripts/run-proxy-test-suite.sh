#!/usr/bin/env bash
# From repo root: bootstrap signed gRPC stack and run modular proxy E2E (proxy-test container).
# Env: PROXY_TEST_SUITE=basic|network|load|full, NETEM_*, STABILITY_SEC, CONCURRENCY_CONNECTIONS, TEST_MODE=local|remote
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p artifacts

export DOCKER_E2E_CONTROL_API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
export PROXY_TEST_SUITE="${PROXY_TEST_SUITE:-basic}"

COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.protocol-bench.yml)

if [[ "${TEST_MODE:-local}" == "remote" ]]; then
  echo "TEST_MODE=remote: expected REMOTE_GRPC_ENDPOINT + REMOTE_CONTROL_API on the runner host." >&2
  echo "Run the stack on the remote host, then:" >&2
  echo "  docker compose ... --profile protocol-bench run --rm -e TEST_MODE=remote \\" >&2
  echo "    -e REMOTE_GRPC_ENDPOINT=... -e REMOTE_CONTROL_API=... proxy-test" >&2
  exit 1
fi

echo "==> build / start postgres + deploy-server"
"${COMPOSE[@]}" up -d --quiet-build postgres deploy-server

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

echo "==> bootstrap control-api gRPC key"
"${COMPOSE[@]}" --profile protocol-bench run --rm bootstrap-grpc-keys

echo "==> restart deploy-server to reload authorized peers"
"${COMPOSE[@]}" restart deploy-server
sleep 4

echo "==> build runtime and start test dependencies"
"${COMPOSE[@]}" --profile protocol-bench build -q proxy-test
"${COMPOSE[@]}" --profile protocol-bench up -d control-api bench-upstream deny-upstream

echo "==> wait for control-api health (host port ${DOCKER_E2E_CONTROL_API_PORT})"
for _ in $(seq 1 90); do
  if curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok; then
    break
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok

echo "==> run proxy-test container (suite=${PROXY_TEST_SUITE})"
"${COMPOSE[@]}" --profile protocol-bench run --rm \
  -e PROXY_TEST_SUITE \
  -e NETEM_DELAY_MS \
  -e NETEM_LOSS_PERCENT \
  -e NETEM_BANDWIDTH \
  -e PROXY_TEST_RUNS \
  -e PROXY_TEST_BW_BYTES \
  -e STABILITY_SEC \
  -e CONCURRENCY_CONNECTIONS \
  -e MAX_CONCURRENCY_CONNECTIONS \
  -e MIN_SUCCESS_RATE \
  -e MAX_CONNECTION_FAILURE_RATE \
  -e RUN_FAILURE_TESTS \
  -e CONTROL_API_LOGIN_USER \
  -e CONTROL_API_LOGIN_PASSWORD \
  proxy-test

echo "OK: proxy test suite finished (report under ./artifacts/)"
