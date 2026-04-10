#!/usr/bin/env bash
# Bootstrap phase 6 stack: PostgreSQL (optional), deploy-server, control-api, build UI.
# Requires: Rust, Node/npm for frontend, PostgreSQL client tools optional.
#
# Usage:
#   export DATABASE_URL='postgresql://user:pass@[::1]:5432/deploy'
#   ./scripts/bootstrap-phase6.sh
#
# Or without DB (API history empty; status still works via gRPC):
#   ./scripts/bootstrap-phase6.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PORT="${PORT:-50051}"
API_PORT="${API_PORT:-8080}"
DEPLOY_ROOT="${DEPLOY_ROOT:-${TMPDIR:-/tmp}/deploy-phase6}"
ENDPOINT="http://[::1]:${PORT}"
GRPC_ENDPOINT="${GRPC_ENDPOINT:-$ENDPOINT}"

mkdir -p "$DEPLOY_ROOT"

echo "==> cargo build (workspace)"
cargo build -q -p deploy-server -p control-api

echo "==> npm build frontend"
if command -v npm >/dev/null 2>&1; then
  (cd "$REPO_ROOT/server-stack/frontend" && npm install --silent && npm run build)
  echo "    UI dist: $REPO_ROOT/server-stack/frontend/dist"
else
  echo "    npm not found; skip frontend build"
fi

echo "==> deploy root: $DEPLOY_ROOT"
echo "==> gRPC:        $GRPC_ENDPOINT"
echo "==> control-api: http://[::1]:${API_PORT}"
echo ""
echo "Run in separate terminals (or use systemd units under server-stack/deploy/systemd/):"
echo ""
echo "  # 1) PostgreSQL must be running if DATABASE_URL is set; create DB and user first."
echo "  export DATABASE_URL='${DATABASE_URL:-postgresql://deploy:deploy@[::1]:5432/deploy}'"
echo ""
echo "  RUST_LOG=info cargo run -p deploy-server -- --root \"$DEPLOY_ROOT\" -p $PORT --database-url \"\$DATABASE_URL\""
echo ""
echo "  RUST_LOG=info DEPLOY_ROOT=\"$DEPLOY_ROOT\" GRPC_ENDPOINT=\"$GRPC_ENDPOINT\" \\"
echo "    cargo run -p control-api -- --deploy-root \"$DEPLOY_ROOT\" --listen-port $API_PORT --database-url \"\$DATABASE_URL\""
echo ""
echo "  # 3) Optional: point control-api at an nginx config file for UI edit + reload:"
echo "  export NGINX_CONFIG_PATH=/etc/nginx/nginx.conf"
echo "  export NGINX_TEST_FULL_CONFIG=true   # if that file is a full nginx.conf"
echo "  export NGINX_ADMIN_TOKEN=secret     # optional; PUT requires Bearer token"
echo ""
echo "  # 4) Optional: nginx -c server-stack/deploy/nginx.conf.example (after editing root)"
echo ""
echo "  # Vite dev (npm run dev) uses another origin — set for control-api:"
echo "  #   CONTROL_API_CORS_ALLOW_ANY=1"
echo ""
echo "Open UI: http://[::1]:${API_PORT}/api/v1/status (or via nginx on :80)"
