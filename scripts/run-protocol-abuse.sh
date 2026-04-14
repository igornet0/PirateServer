#!/usr/bin/env bash
# Limited TCP abuse probes (merge: test + protocol-ext).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export ABUSE_TCP_HALF_OPEN="${ABUSE_TCP_HALF_OPEN:-5}"
COMPOSE=(docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.protocol-ext.yml)

"${COMPOSE[@]}" up -d --build --quiet-build postgres deploy-server
for _ in $(seq 1 60); do
  if "${COMPOSE[@]}" exec -T deploy-server test -f /data/.keys/server_ed25519.json 2>/dev/null; then
    break
  fi
  sleep 1
done

"${COMPOSE[@]}" --profile protocol-abuse build -q protocol-abuse
"${COMPOSE[@]}" --profile protocol-abuse run --rm protocol-abuse

echo "OK: protocol-abuse finished"
