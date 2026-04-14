#!/usr/bin/env bash
# Fuzz-style unary calls (merge: test + protocol-ext). Fast; no pairing required.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export FUZZ_ITERATIONS="${FUZZ_ITERATIONS:-40}"
COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.protocol-ext.yml)

echo "==> start postgres + deploy-server"
"${COMPOSE[@]}" up -d --build --quiet-build postgres deploy-server

for _ in $(seq 1 60); do
  if "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json 2>/dev/null; then
    break
  fi
  sleep 1
done

echo "==> build + run protocol-fuzz"
"${COMPOSE[@]}" --profile protocol-fuzz build -q protocol-fuzz
"${COMPOSE[@]}" --profile protocol-fuzz run --rm protocol-fuzz

echo "OK: protocol-fuzz finished"
