#!/usr/bin/env bash
# Собрать тестовый стек в Docker и прогнать scripts/test-anti-ddos.sh с контейнера anti-ddos-runner на nginx (между контейнерами).
# Только свои стенды / с разрешением владельца.
#
# Из корня репозитория:
#   ./scripts/run-docker-anti-ddos.sh
#   ./scripts/run-docker-anti-ddos.sh --pc-only
#   ./scripts/run-docker-anti-ddos.sh --down
#   PARALLEL=20 BURST_N=100 ./scripts/run-docker-anti-ddos.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

COMPOSE_FILES=(
  -f tests/docker/docker-compose.test.yml
  -f tests/docker/docker-compose.anti-ddos.yml
)
PROJECT="${COMPOSE_PROJECT_NAME:-pirateserver-antiddos}"
COMPOSE=(docker compose "${COMPOSE_FILES[@]}" -p "$PROJECT")

PC_ONLY=0
DOWN=0
for arg in "$@"; do
  case "$arg" in
    --pc-only) PC_ONLY=1 ;;
    --down) DOWN=1 ;;
  esac
done

if [[ "$DOWN" == "1" ]]; then
  "${COMPOSE[@]}" down -v
  exit 0
fi

"${COMPOSE[@]}" up -d --build

echo ""
echo "==> Smoke: контейнер «ПК» (client + curl через nginx)"
"${COMPOSE[@]}" --profile anti-ddos run --rm pc-client-smoke

if [[ "$PC_ONLY" == "1" ]]; then
  echo ""
  echo "==> --pc-only: пропуск anti-ddos-runner"
  exit 0
fi

echo ""
echo "==> Anti-DDoS harness: контейнер → nginx:80 (внутренняя сеть Docker)"
"${COMPOSE[@]}" --profile anti-ddos run --rm anti-ddos-runner

echo ""
echo "OK: Docker anti-ddos run finished (project: $PROJECT). Teardown: COMPOSE_PROJECT_NAME=$PROJECT ./scripts/run-docker-anti-ddos.sh --down"
