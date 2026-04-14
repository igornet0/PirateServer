#!/usr/bin/env bash
# Run inside the e2e container (see tests/docker/docker-compose.test.yml) or invoke from host via scripts/run-docker-e2e.sh
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
NGINX="${nginx_public:-http://nginx:80}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"

chmod +x /fixtures/minimal-app/run.sh 2>/dev/null || true

curl_api() {
  local opts=(-sf)
  if [[ -n "${DOCKER_E2E_API_TOKEN:-}" ]]; then
    opts+=(-H "Authorization: Bearer ${DOCKER_E2E_API_TOKEN}")
  fi
  curl "${opts[@]}" "$@"
}

if [[ -n "${DOCKER_E2E_API_TOKEN:-}" ]]; then
  echo "==> bearer: /api/v1/status without token returns 401"
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "$API/api/v1/status" || true)
  if [[ "$CODE" != "401" ]]; then
    echo "expected 401, got ${CODE}"
    exit 1
  fi
fi

echo "==> health (direct control-api)"
curl -sf "$API/health" | grep -q ok

echo "==> status (direct)"
curl_api "$API/api/v1/status"
echo ""

echo "==> status (via nginx proxy)"
curl_api "$NGINX/api/v1/status"
echo ""

echo "==> deploy v-e2e-1"
client --endpoint "$GRPC" deploy /fixtures/minimal-app --release v-e2e-1

echo "==> status after first deploy"
curl_api "$API/api/v1/status" | grep -q '"state":"running"'

echo "==> releases lists v-e2e-1"
REL=$(curl_api "$API/api/v1/releases")
echo "$REL" | grep -q v-e2e-1

echo "==> deploy v-e2e-2"
client --endpoint "$GRPC" deploy /fixtures/minimal-app --release v-e2e-2

echo "==> current version v-e2e-2"
curl_api "$API/api/v1/status" | grep -q '"current_version":"v-e2e-2"'

echo "==> rollback to v-e2e-1"
client --endpoint "$GRPC" rollback v-e2e-1

echo "==> current version v-e2e-1 after rollback"
curl_api "$API/api/v1/status" | grep -q '"current_version":"v-e2e-1"'

echo "==> history has events"
HIST=$(curl_api "$API/api/v1/history")
echo "$HIST" | grep -q '"events":\[{' || {
  echo "$HIST"
  exit 1
}

echo "==> pirate sessions (ListSessions; unsigned gRPC in compose)"
client --endpoint "$GRPC" sessions

echo "==> pirate sessions --last-log (session audit table)"
client --endpoint "$GRPC" sessions --last-log --limit 20 | head -n 8

echo "==> pirate sessions --export-log (CSV)"
client --endpoint "$GRPC" sessions --export-log -o /tmp/pirate-sessions-e2e.csv
grep -q '^id,created_at_ms,kind,' /tmp/pirate-sessions-e2e.csv

if [[ "${NGINX_E2E_TESTS:-}" == "1" ]]; then
  echo "==> nginx config API: GET (enabled)"
  GET=$(curl_api "$API/api/v1/nginx/config")
  echo "$GET" | grep -q '"enabled":true' || {
    echo "$GET"
    exit 1
  }
  echo "==> nginx config API: PUT (reload)"
  BODY=$(jq -Rs '{content: .}' </fixtures/nginx-e2e-put.conf)
  RESP=$(curl_api -X PUT "$API/api/v1/nginx/config" -H "Content-Type: application/json" -d "$BODY")
  echo "$RESP" | grep -q '"ok":true' || {
    echo "$RESP"
    exit 1
  }
fi

echo "OK: Docker e2e passed"
