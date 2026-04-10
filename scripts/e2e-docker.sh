#!/usr/bin/env bash
# Run inside the e2e container (see docker-compose.test.yml) or invoke from host via scripts/run-docker-e2e.sh
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
NGINX="${nginx_public:-http://nginx:80}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"

chmod +x /fixtures/minimal-app/run.sh 2>/dev/null || true

echo "==> health (direct control-api)"
curl -sf "$API/health" | grep -q ok

echo "==> status (direct)"
curl -sf "$API/api/v1/status"

echo "==> status (via nginx proxy)"
curl -sf "$NGINX/api/v1/status"

echo "==> deploy v-e2e-1"
client --endpoint "$GRPC" deploy /fixtures/minimal-app --version v-e2e-1

echo "==> status after first deploy"
curl -sf "$API/api/v1/status" | grep -q '"state":"running"'

echo "==> releases lists v-e2e-1"
REL=$(curl -sf "$API/api/v1/releases")
echo "$REL" | grep -q v-e2e-1

echo "==> deploy v-e2e-2"
client --endpoint "$GRPC" deploy /fixtures/minimal-app --version v-e2e-2

echo "==> current version v-e2e-2"
curl -sf "$API/api/v1/status" | grep -q '"current_version":"v-e2e-2"'

echo "==> rollback to v-e2e-1"
client --endpoint "$GRPC" rollback v-e2e-1

echo "==> current version v-e2e-1 after rollback"
curl -sf "$API/api/v1/status" | grep -q '"current_version":"v-e2e-1"'

echo "==> history has events"
HIST=$(curl -sf "$API/api/v1/history")
echo "$HIST" | grep -q '"events":\[{' || {
  echo "$HIST"
  exit 1
}

echo "OK: Docker e2e passed"
