#!/usr/bin/env bash
# Smoke-test control-api on published host ports (same checks as the tail of run-docker-e2e.sh).
#
# Requires the Docker test stack to be RUNNING (e.g. after `docker compose … up -d`, not after `… down`).
# If you only ran `./scripts/run-docker-e2e.sh --down`, containers are removed — bring the stack up first.
#
# Requires: curl, jq
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="tests/docker/docker-compose.test.yml"

HOST="${DOCKER_E2E_HOST:-127.0.0.1}"
API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"
NGINX_PORT="${DOCKER_E2E_DASHBOARD_PORT:-18080}"
API="http://${HOST}:${API_PORT}"
NGINX="http://${HOST}:${NGINX_PORT}"

fail_no_stack() {
  echo "" >&2
  echo "error: cannot reach ${API}/health — the Docker e2e stack is probably not running." >&2
  echo "Start it (from repo root), then re-run this script:" >&2
  echo "  cd \"${ROOT}\" && docker compose -f ${COMPOSE_FILE} up -d --build" >&2
  echo "Or use the full runner (starts stack, runs e2e container, then this smoke):" >&2
  echo "  ./scripts/run-docker-e2e.sh" >&2
  exit 1
}

echo "==> host smoke: health $API"
if ! OUT=$(curl -sf --connect-timeout 3 --max-time 15 "$API/health" 2>&1); then
  echo "curl: ${OUT:-connection failed}" >&2
  fail_no_stack
fi
if ! echo "$OUT" | grep -q ok; then
  echo "error: expected health body 'ok', got: $OUT" >&2
  exit 1
fi

echo "==> host smoke: login $API → Bearer for /api/v1/status"
LOGIN=$(curl -sf -X POST "$API/api/v1/auth/login" \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"testpass"}') || {
  echo "error: POST /api/v1/auth/login failed (wrong credentials or JWT not configured?)" >&2
  exit 1
}
TOKEN=$(echo "$LOGIN" | jq -r '.access_token')
if [[ -z "$TOKEN" || "$TOKEN" == "null" ]]; then
  echo "$LOGIN" >&2
  echo "error: no access_token in login response" >&2
  exit 1
fi

echo "==> host smoke: status (direct)"
curl -sf -H "Authorization: Bearer ${TOKEN}" "$API/api/v1/status" | jq -e '.current_version' >/dev/null

echo "==> host smoke: status (via nginx $NGINX)"
curl -sf -H "Authorization: Bearer ${TOKEN}" "$NGINX/api/v1/status" | jq -e '.current_version' >/dev/null

echo "OK: host smoke passed (use these URLs from desktop on this host; for another PC set DEPLOY_CONTROL_API_PUBLIC_URL / DIRECT on the server to match how that machine reaches nginx or control-api)."
