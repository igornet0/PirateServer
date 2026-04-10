#!/usr/bin/env bash
# Local end-to-end: deploy-server (IPv6) + client deploy / status / rollback.
#
# Prerequisites: Rust toolchain, free TCP port (default 50051), non-root user.
#
# Usage (from repo root):
#   chmod +x scripts/local-e2e.sh
#   ./scripts/local-e2e.sh
#
# Optional:
#   PORT=50052 ./scripts/local-e2e.sh
#   RUST_LOG=info ./scripts/local-e2e.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PORT="${PORT:-50051}"
ENDPOINT="http://[::1]:${PORT}"
DEPLOY_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/deploy-e2e.XXXXXX")"
BUILD_DIR="$REPO_ROOT/examples/test-app/build"

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "==> deploy root: $DEPLOY_ROOT"
echo "==> endpoint:    $ENDPOINT"
echo "==> build dir:   $BUILD_DIR"

if [[ ! -d "$BUILD_DIR" ]]; then
  echo "missing $BUILD_DIR" >&2
  exit 1
fi

chmod +x "$BUILD_DIR/run.sh" 2>/dev/null || true

echo "==> cargo build (workspace)"
cargo build -q -p deploy-server -p deploy-client --bin client

echo "==> start deploy-server"
RUST_LOG="${RUST_LOG:-info}" \
  cargo run -q -p deploy-server -- \
    --root "$DEPLOY_ROOT" \
    -p "$PORT" \
  &
SERVER_PID=$!

echo "==> wait for server (pid $SERVER_PID)"
sleep 1
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "server exited early" >&2
  exit 1
fi

run_client() {
  cargo run -q -p deploy-client --bin client -- --endpoint "$ENDPOINT" "$@"
}

echo "==> client deploy v1"
run_client deploy "$BUILD_DIR" --version v1

echo "==> client status (expect v1)"
run_client status

echo "==> client deploy v2"
run_client deploy "$BUILD_DIR" --version v2

echo "==> client status (expect v2)"
run_client status

echo "==> client rollback v1"
run_client rollback v1

echo "==> client status (expect v1)"
run_client status

echo "==> OK: deploy + status + rollback completed"
echo "    (temp deploy root kept until exit: $DEPLOY_ROOT)"
