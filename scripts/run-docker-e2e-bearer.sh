#!/usr/bin/env bash
# Same as run-docker-e2e.sh but merges tests/docker/docker-compose.bearer-override.yml (CONTROL_API_BEARER_TOKEN).
# Requires DOCKER_E2E_API_TOKEN in the environment.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export DOCKER_E2E_API_TOKEN="${DOCKER_E2E_API_TOKEN:?Set DOCKER_E2E_API_TOKEN for bearer e2e}"
COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.bearer-override.yml)

"${COMPOSE[@]}" up -d --build
bash "${ROOT}/scripts/print-docker-connection.sh"
"${COMPOSE[@]}" --profile e2e run --rm e2e
bash "${ROOT}/scripts/e2e-docker-host-smoke.sh"

if [[ "${1:-}" == "--down" ]]; then
  "${COMPOSE[@]}" down -v
fi
