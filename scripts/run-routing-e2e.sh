#!/usr/bin/env bash
# Собрать стек из tests/docker/docker-compose.test.yml + routing-e2e и прогнать проверки правил.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.routing-e2e.yml --profile routing-e2e)

echo "==> docker compose build (test runtime)"
"${COMPOSE[@]}" build deploy-server

echo "==> run routing-e2e"
"${COMPOSE[@]}" run --rm routing-e2e

echo "==> done"
