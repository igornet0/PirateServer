#!/usr/bin/env bash
# Run inside the e2e container (see tests/docker/docker-compose.test.yml) or invoke from host via scripts/run-docker-e2e.sh
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
NGINX="${nginx_public:-http://nginx:80}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
EXP_PUB="${E2E_EXPECT_CONTROL_API_PUBLIC:-http://nginx:80}"
EXP_DIR="${E2E_EXPECT_CONTROL_API_DIRECT:-http://control-api:8080}"

chmod +x /fixtures/minimal-app/run.sh 2>/dev/null || true

# Prefer static bearer (bearer-override CI) over JWT from login.
E2E_ACCESS_TOKEN=""
curl_api() {
  local opts=(-sf)
  if [[ -n "${DOCKER_E2E_API_TOKEN:-}" ]]; then
    opts+=(-H "Authorization: Bearer ${DOCKER_E2E_API_TOKEN}")
  elif [[ -n "${E2E_ACCESS_TOKEN:-}" ]]; then
    opts+=(-H "Authorization: Bearer ${E2E_ACCESS_TOKEN}")
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

# JWT login (CONTROL_API_JWT_SECRET + seeded admin in Postgres). Skipped when using static DOCKER_E2E_API_TOKEN only.
if [[ -z "${DOCKER_E2E_API_TOKEN:-}" ]]; then
  echo "==> auth/login (direct) — JWT for subsequent /api/v1/* calls"
  LOGIN_JSON=$(curl -sf -X POST "$API/api/v1/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"username":"admin","password":"testpass"}') || {
    echo "login failed (check CONTROL_API_JWT_SECRET + deploy-server admin seed)"
    exit 1
  }
  E2E_ACCESS_TOKEN=$(echo "$LOGIN_JSON" | jq -r '.access_token // empty')
  if [[ -z "$E2E_ACCESS_TOKEN" || "$E2E_ACCESS_TOKEN" == "null" ]]; then
    echo "$LOGIN_JSON"
    echo "expected .access_token from login"
    exit 1
  fi
  echo "==> auth/login (via nginx)"
  NGX_LOGIN=$(curl -sf -X POST "$NGINX/api/v1/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"username":"admin","password":"testpass"}')
  echo "$NGX_LOGIN" | jq -e --arg t "$E2E_ACCESS_TOKEN" '(.access_token == $t)' >/dev/null
fi

echo "==> grpc GetStatus — control_api_http_url / control_api_http_url_direct (desktop hints)"
GRPC_ADDR="${GRPC#http://}"
GRPC_ADDR="${GRPC_ADDR#https://}"
GS=$(grpcurl -plaintext \
  -import-path /proto \
  -proto deploy.proto \
  -d '{"projectId":"default"}' \
  "$GRPC_ADDR" \
  deploy.DeployService/GetStatus)
echo "$GS" | jq -e --arg p "$EXP_PUB" --arg d "$EXP_DIR" \
  '(.controlApiHttpUrl == $p) and (.controlApiHttpUrlDirect == $d)' >/dev/null

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
