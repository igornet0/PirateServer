#!/usr/bin/env bash
# Full stack + control-api image with nginx (for /api/v1/nginx/config integration tests).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.nginx-e2e.yml)

"${COMPOSE[@]}" up -d --build
bash "${ROOT}/scripts/print-docker-connection.sh"
"${COMPOSE[@]}" --profile e2e run --rm -e NGINX_E2E_TESTS=1 e2e
bash "${ROOT}/scripts/e2e-docker-host-smoke.sh"

if [[ "${1:-}" == "--down" ]]; then
  "${COMPOSE[@]}" down -v
fi
