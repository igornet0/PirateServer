#!/usr/bin/env bash
# Run inside grpc-client-e2e (see docker-compose.grpc-client-e2e.yml): Pair, ProxyTunnel via board, signed deploys.
set -euo pipefail

GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
API="${CONTROL_API_DIRECT:-http://control-api:8080}"
RODATA="${DEPLOY_DEPLOY_ROOT_READONLY:-/mnt/rodata}"

chmod +x /fixtures/minimal-app/run.sh 2>/dev/null || true

wait_tcp() {
  local host=$1
  local port=$2
  local max=90
  local i=0
  while ! bash -c "exec 3<>/dev/tcp/${host}/${port}" 2>/dev/null; do
    i=$((i + 1))
    if [[ "$i" -ge "$max" ]]; then
      echo "timeout waiting for ${host}:${port}"
      exit 1
    fi
    sleep 1
  done
}

echo "==> wait deploy-server gRPC"
wait_tcp deploy-server 50051

echo "==> install bundle (read-only deploy root + keys)"
if [[ ! -d "${RODATA}/.keys" ]]; then
  echo "missing ${RODATA}/.keys (deploy-server not initialized?)"
  exit 1
fi
BUNDLE_FILE=$(mktemp)
CFG="/tmp/pirate-grpc-e2e-$$"
export XDG_CONFIG_HOME="$CFG"
trap 'rm -f "$BUNDLE_FILE"; rm -rf "$CFG"' EXIT
mkdir -p "$XDG_CONFIG_HOME"

DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root "$RODATA" print-install-bundle >"$BUNDLE_FILE"

echo "==> client auth (pair + GetStatus)"
client auth "$BUNDLE_FILE"

echo "==> client status (signed)"
client status

echo "==> board + CONNECT proxy to control-api:8080"
client board --url "$GRPC" --listen 127.0.0.1:3128 &
BOARD_PID=$!
sleep 2
curl --proxytunnel --proxy http://127.0.0.1:3128 -sf "$API/health" | grep -q ok
kill "$BOARD_PID"
wait "$BOARD_PID" 2>/dev/null || true

curl_api() {
  curl -sf "$@"
}

echo "==> deploy v-grpc-1 (signed)"
client deploy /fixtures/minimal-app --release v-grpc-1

echo "==> status after first deploy"
curl_api "$API/api/v1/status" | grep -q '"state":"running"'

echo "==> deploy v-grpc-2 (update)"
client deploy /fixtures/minimal-app --release v-grpc-2

echo "==> current version v-grpc-2"
curl_api "$API/api/v1/status" | grep -q '"current_version":"v-grpc-2"'

echo "OK: grpc-client Docker e2e passed"
