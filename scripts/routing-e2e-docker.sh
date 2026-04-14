#!/usr/bin/env bash
# Запускается внутри контейнера routing-e2e (tests/docker/docker-compose.routing-e2e.yml).
set -euo pipefail

GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
RODATA="${DEPLOY_DEPLOY_ROOT_READONLY:-/mnt/rodata}"

wait_tcp() {
  local host=$1
  local port=$2
  local max=90
  local i=0
  while ! bash -c "exec 3<>/dev/tcp/${host}/${port}" 2>/dev/null; do
    i=$((i + 1))
    if [[ "$i" -ge "$max" ]]; then
      echo "timeout waiting for ${host}:${port}" >&2
      exit 1
    fi
    sleep 1
  done
}

echo "==> wait deploy-server gRPC"
wait_tcp deploy-server 50051

echo "==> wait for server keys in deploy volume"
for _ in $(seq 1 90); do
  if [[ -f "${RODATA}/.keys/server_ed25519.json" ]]; then
    break
  fi
  sleep 1
done
if [[ ! -f "${RODATA}/.keys/server_ed25519.json" ]]; then
  echo "timeout: ${RODATA}/.keys/server_ed25519.json missing" >&2
  exit 1
fi

export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-routing-e2e-$$}"
CFG="${XDG_CONFIG_HOME}/pirate-client"
mkdir -p "$CFG"
BUNDLE_FILE=$(mktemp)

cp /fixtures/routing/settings.json "$CFG/settings.json"
cp /fixtures/routing/rules-block.json "$CFG/"
cp /fixtures/routing/rules-pass.json "$CFG/"

BOARD_PID=""

on_exit() {
  if [[ -n "${BOARD_PID}" ]]; then
    kill "$BOARD_PID" 2>/dev/null || true
    wait "$BOARD_PID" 2>/dev/null || true
  fi
  rm -f "$BUNDLE_FILE"
  rm -rf "${XDG_CONFIG_HOME:-}"
}
trap on_exit EXIT

echo "==> install bundle + client auth"
DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root "$RODATA" print-install-bundle >"$BUNDLE_FILE"
client auth "$BUNDLE_FILE"

echo "==> start board (CONNECT proxy)"
client --url "$GRPC" board --listen 127.0.0.1:3128 --settings "$CFG/settings.json" &
BOARD_PID=$!
sleep 3

echo "==> case: JSON _block → CONNECT 403 (test.block.local)"
set +e
curl -k -sS -m 25 -v -x http://127.0.0.1:3128 https://test.block.local/ -o /dev/null 2> /tmp/routing-block.log
set -e
if ! grep -qE '403|Forbidden' /tmp/routing-block.log; then
  echo "expected HTTP 403 Forbidden from proxy for blocked host" >&2
  cat /tmp/routing-block.log >&2
  exit 1
fi

echo "==> case: JSON _pass → direct HTTPS to nginx (test.pass.local)"
curl -k -sf -m 25 -x http://127.0.0.1:3128 https://test.pass.local/ | grep -q ROUTING_E2E_OK

echo "OK: routing rules Docker e2e passed"
