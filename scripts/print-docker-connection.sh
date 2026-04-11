#!/usr/bin/env bash
# Print connection bundle for the docker-compose.test.yml stack (after `up`).
# Use from host: gRPC client, browser dashboard, optional direct control-api.
set -euo pipefail

HOST="${DOCKER_E2E_HOST:-127.0.0.1}"
GRPC_PORT="${DOCKER_E2E_GRPC_PORT:-50051}"
DASHBOARD_PORT="${DOCKER_E2E_DASHBOARD_PORT:-18080}"
API_PORT="${DOCKER_E2E_CONTROL_API_PORT:-18081}"

echo ""
echo "=== PirateServer Docker test stack — connection bundle ==="
echo "export GRPC_ENDPOINT=http://${HOST}:${GRPC_PORT}"
echo "export DASHBOARD_URL=http://${HOST}:${DASHBOARD_PORT}"
echo "export CONTROL_API_DIRECT=http://${HOST}:${API_PORT}"
echo ""
echo "Local PC: run the deploy client against gRPC, e.g.:"
echo "  client --endpoint \"http://${HOST}:${GRPC_PORT}\" status"
echo ""
if [[ -n "${DOCKER_E2E_API_TOKEN:-}" ]]; then
  echo "control-api Bearer token is set (use with curl / HTTP clients):"
  echo "  export CONTROL_API_BEARER_TOKEN=\"\${DOCKER_E2E_API_TOKEN}\""
  echo "  curl -H \"Authorization: Bearer \${DOCKER_E2E_API_TOKEN}\" \"\${CONTROL_API_DIRECT}/api/v1/status\""
  echo ""
else
  echo "HTTP /api/v1/* has no Bearer requirement unless you start the stack with"
  echo "  docker compose -f docker-compose.test.yml -f docker-compose.bearer-override.yml up -d"
  echo "  (set DOCKER_E2E_API_TOKEN in the environment or .env)."
  echo ""
fi
echo "Note: this stack sets DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1 for e2e. Production: omit it, use install"
echo "  bundle JSON (token, url, pairing) and \`client pair\` / desktop Connect (see docs/GRPC_AUTH_FUTURE.md)."
echo "========================================================"
echo ""
