#!/usr/bin/env sh
# Regenerate sing-box config from DATABASE_URL and optionally reload (set INGRESS_RELOAD_CMD).
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
OUT="${INGRESS_OUTPUT:-/etc/sing-box/config.json}"
exec cargo run --quiet -p ingress-config --bin ingress-manager -- \
  --database-url "$DATABASE_URL" \
  --output "$OUT" \
  ${INGRESS_RELOAD_CMD:+--reload-cmd "$INGRESS_RELOAD_CMD"}
