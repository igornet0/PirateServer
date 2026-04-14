#!/usr/bin/env bash
# Fail fast if nothing listens on HOST:PORT (gRPC from tests/docker/docker-compose.test.yml).
# Usage: scripts/docker-grpc-preflight.sh [HOST] [PORT]
set -euo pipefail

HOST="${1:-127.0.0.1}"
PORT="${2:-50051}"

port_open() {
  if command -v nc >/dev/null 2>&1; then
    nc -z "$HOST" "$PORT" 2>/dev/null
    return $?
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 -c "
import socket, sys
s = socket.socket()
s.settimeout(1)
try:
    s.connect(('$HOST', $PORT))
    sys.exit(0)
except OSError:
    sys.exit(1)
" 2>/dev/null
    return $?
  fi
  if bash -c "exec 3<>/dev/tcp/$HOST/$PORT" 2>/dev/null; then
    exec 3<&- 3>&- 2>/dev/null || true
    return 0
  fi
  return 1
}

if port_open; then
  exit 0
fi

echo "error: no TCP listener on ${HOST}:${PORT} (deploy-server gRPC)." >&2
echo "" >&2
echo "  Start the stack from repo root:" >&2
echo "    make -f Makefile.docker up" >&2
echo "" >&2
echo "  Then check:" >&2
echo "    make -f Makefile.docker ps" >&2
echo "    make -f Makefile.docker logs SERVICE=deploy-server" >&2
echo "" >&2
echo "  Custom host/port: DOCKER_E2E_HOST / DOCKER_E2E_GRPC_PORT" >&2
exit 1
