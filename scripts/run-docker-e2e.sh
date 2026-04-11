#!/usr/bin/env bash
# From repo root: build stack, run e2e container, optional teardown.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
COMPOSE=(docker compose -f docker-compose.test.yml)

"${COMPOSE[@]}" up -d --build
# shellcheck source=scripts/print-docker-connection.sh
bash "${ROOT}/scripts/print-docker-connection.sh"
"${COMPOSE[@]}" --profile e2e run --rm e2e

if [[ "${1:-}" == "--down" ]]; then
  "${COMPOSE[@]}" down -v
fi
