#!/usr/bin/env bash
# Docker integration: PostgreSQL + Redis + deploy-server (DEPLOY_REDIS_URL) + metrics + one ProxyTunnel via client.
#
# Usage (repo root):
#   ./scripts/run-redis-tunnel-docker-tests.sh
# Teardown:
#   docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.redis-tunnel-test.yml down -v
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export DOCKER_E2E_CONTROL_API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
export DOCKER_E2E_METRICS_PORT="${DOCKER_E2E_METRICS_PORT:-19090}"
export DOCKER_E2E_REDIS_PORT="${DOCKER_E2E_REDIS_PORT:-16379}"

COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.redis-tunnel-test.yml)

echo "==> build / start postgres + redis + deploy-server"
"${COMPOSE[@]}" up -d --build --quiet-build postgres redis deploy-server

echo "==> wait for deploy-server keys"
for _ in $(seq 1 90); do
  if "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json 2>/dev/null; then
    break
  fi
  sleep 1
done
if ! "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json; then
  echo "timeout: server keys missing" >&2
  exit 1
fi

echo "==> bootstrap control-api gRPC key (writes /data/.keys/control_api_ed25519.json)"
"${COMPOSE[@]}" --profile redis-tunnel-test run --rm bootstrap-grpc-keys

echo "==> restart deploy-server so it reloads authorized_peers.json"
"${COMPOSE[@]}" restart deploy-server
sleep 4

echo "==> start bench-upstream + control-api"
"${COMPOSE[@]}" up -d --quiet-build bench-upstream control-api

echo "==> wait for control-api health"
for _ in $(seq 1 90); do
  if curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok; then
    break
  fi
  sleep 1
done
curl -sf "http://127.0.0.1:${DOCKER_E2E_CONTROL_API_PORT}/health" | grep -q ok

echo "==> Redis PING"
"${COMPOSE[@]}" exec -T redis redis-cli ping | grep -q PONG

echo "==> deploy-server Prometheus metrics (tunnel counters)"
curl -sf "http://127.0.0.1:${DOCKER_E2E_METRICS_PORT}/metrics" | grep -qE '^# HELP deploy_proxy_tunnels_(total|open)'

TUNNELS_BEFORE="$(curl -sf "http://127.0.0.1:${DOCKER_E2E_METRICS_PORT}/metrics" | sed -n 's/^deploy_proxy_tunnels_total \([0-9][0-9]*\).*/\1/p' | head -1)"
TUNNELS_BEFORE="${TUNNELS_BEFORE:-0}"

echo "==> install bundle for inner runner"
# docker compose exec runs as root by default; deploy-server refuses UID 0 (ensure_not_root).
BUNDLE_JSON="$("${COMPOSE[@]}" exec -T -u deploy deploy-server \
  deploy-server --root /data print-install-bundle)"
if ! echo "$BUNDLE_JSON" | jq -e . >/dev/null 2>&1; then
  echo "expected JSON install bundle" >&2
  exit 1
fi
export BUNDLE_JSON

echo "==> run tunnel smoke (client in redis-tunnel-runner)"
"${COMPOSE[@]}" --profile redis-tunnel-test run --rm \
  -e BUNDLE_JSON="$BUNDLE_JSON" \
  redis-tunnel-runner

echo "==> Redis: no leftover tunnel keys after unregister (best-effort)"
KEYS_OUT="$("${COMPOSE[@]}" exec -T redis redis-cli KEYS 'proxy:*' 2>/dev/null || true)"
if [[ -n "${KEYS_OUT//[$'\t\r\n ']/}" ]]; then
  echo "WARN: redis still has keys: $KEYS_OUT" >&2
else
  echo "OK: redis KEYS proxy:* empty"
fi

TUNNELS_AFTER="$(curl -sf "http://127.0.0.1:${DOCKER_E2E_METRICS_PORT}/metrics" | sed -n 's/^deploy_proxy_tunnels_total \([0-9][0-9]*\).*/\1/p' | head -1)"
TUNNELS_AFTER="${TUNNELS_AFTER:-0}"
if [[ "$TUNNELS_AFTER" -le "$TUNNELS_BEFORE" ]]; then
  echo "WARN: deploy_proxy_tunnels_total did not increase (before=$TUNNELS_BEFORE after=$TUNNELS_AFTER)" >&2
else
  echo "OK: deploy_proxy_tunnels_total increased ($TUNNELS_BEFORE → $TUNNELS_AFTER)"
fi

echo "OK: redis tunnel docker tests finished"
