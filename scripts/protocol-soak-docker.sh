#!/usr/bin/env bash
# Long loop of client status — manual/nightly soak.
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-protocol-soak-config}"
CFG_DIR="${XDG_CONFIG_HOME}/pirate-client"
mkdir -p "$CFG_DIR"

SOAK_DURATION_SEC="${SOAK_DURATION_SEC:-300}"
SOAK_INTERVAL_SEC="${SOAK_INTERVAL_SEC:-10}"

echo "==> protocol-soak SOAK_DURATION_SEC=$SOAK_DURATION_SEC SOAK_INTERVAL_SEC=$SOAK_INTERVAL_SEC"

curl -sf "$API/health" | grep -q ok

BUNDLE="$(env DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=0 DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root /data print-install-bundle)"
client --endpoint "$GRPC" auth "$BUNDLE"

start_ts=$(date +%s)
iter=0
while true; do
  now=$(date +%s)
  if [[ $((now - start_ts)) -ge "$SOAK_DURATION_SEC" ]]; then
    break
  fi
  iter=$((iter + 1))
  client --endpoint "$GRPC" status >/dev/null 2>&1 || {
    echo "soak: client status failed at iter=$iter" >&2
    exit 1
  }
  echo "soak iter=$iter (elapsed $((now - start_ts))s / ${SOAK_DURATION_SEC}s)"
  sleep "$SOAK_INTERVAL_SEC"
done

echo "OK: protocol-soak finished ($iter iterations)"
