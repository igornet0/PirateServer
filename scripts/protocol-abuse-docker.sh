#!/usr/bin/env bash
# Limited adversarial probes (half-open style TCP) — does not replace full security audit.
set -euo pipefail

ABUSE_TCP_HALF_OPEN="${ABUSE_TCP_HALF_OPEN:-5}"
HOST="${DEPLOY_SERVER_HOST:-deploy-server}"
PORT="${DEPLOY_SERVER_PORT:-50051}"

echo "==> protocol-abuse ABUSE_TCP_HALF_OPEN=$ABUSE_TCP_HALF_OPEN (bash /dev/tcp to $HOST:$PORT)"

for i in $(seq 1 "$ABUSE_TCP_HALF_OPEN"); do
  timeout 0.4 bash -c "echo >/dev/tcp/$HOST/$PORT" 2>/dev/null || true
done

sleep 1
echo "==> optional metrics smoke (if DEPLOY_METRICS_BIND on deploy-server)"
if curl -sf "http://${HOST}:9090/metrics" >/dev/null 2>&1; then
  echo "metrics: OK"
else
  echo "metrics: skip (no listener on ${HOST}:9090)"
fi

echo "OK: protocol-abuse finished (best-effort TCP probes)"
