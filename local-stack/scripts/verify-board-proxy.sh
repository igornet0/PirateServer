#!/usr/bin/env bash
# Reproduces the "verify traffic through pirate board" checklist from the project plan.
# Prerequisites: `pirate auth` for your gRPC host; `pirate board` listening (default 127.0.0.1:3128).
#
# Usage:
#   export PIRATE=/path/to/pirate   # optional; default: `pirate` from PATH
#   export CONTROL_API=http://192.168.0.30:8080
#   export BOARD_PROXY=http://127.0.0.1:3128
#   export HTTPS_TEST_URL=https://api.ipify.org
#   ./scripts/verify-board-proxy.sh
#
set -euo pipefail

PIRATE="${PIRATE:-pirate}"
CONTROL_API="${CONTROL_API:-http://192.168.0.30:8080}"
BOARD_PROXY="${BOARD_PROXY:-http://127.0.0.1:3128}"
HTTPS_TEST_URL="${HTTPS_TEST_URL:-https://api.ipify.org}"

die() { echo "error: $*" >&2; exit 1; }

if [[ "$PIRATE" == */* ]] || [[ "$PIRATE" == .* ]]; then
  [[ -f "$PIRATE" ]] || die "pirate binary not found: $PIRATE"
else
  command -v "$PIRATE" >/dev/null 2>&1 || die "pirate not found (set PIRATE= to your binary or install to PATH)"
fi

if [[ "${SKIP_BOARD_LISTEN_CHECK:-}" != "1" ]]; then
  hostport="${BOARD_PROXY#http://}"
  hostport="${hostport#https://}"
  host="${hostport%%:*}"
  port="${hostport##*:}"
  [[ "$host" == "$port" ]] && port=3128
  if ! command -v nc >/dev/null 2>&1; then
    echo "note: install \`nc\` (netcat) for listen check, or set SKIP_BOARD_LISTEN_CHECK=1" >&2
  else
    nc -z -w 2 "$host" "$port" 2>/dev/null || die "nothing listening on $host:$port — start \`pirate board\` first (or set SKIP_BOARD_LISTEN_CHECK=1)"
  fi
fi

echo "=== 1) Direct control-api ping (does NOT use pirate board) ==="
"$PIRATE" ping --http-url "$CONTROL_API" --bytes 0 || true

echo
echo "=== 2) Same ping through HTTP CONNECT → pirate board ==="
"$PIRATE" test-proxy ping --http-url "$CONTROL_API" --proxy "$BOARD_PROXY" --bytes 0 || true

echo
echo "=== 3) curl: HTTPS via CONNECT to board (look for: CONNECT … HTTP/1.1 or Trying …:3128) ==="
if ! command -v curl >/dev/null 2>&1; then
  echo "skip: curl not installed" >&2
else
  set +e
  _curl_log="$(
    HTTPS_PROXY="$BOARD_PROXY" HTTP_PROXY="$BOARD_PROXY" \
      curl -fsS -o /dev/null -v --max-time 25 "$HTTPS_TEST_URL" 2>&1
  )"
  _curl_st=$?
  set -e
  echo "$_curl_log" | head -40
  if [[ "$_curl_st" -ne 0 ]]; then
    echo "(curl exited $_curl_st; if board is not running, expect connection refused — still confirms curl uses HTTPS_PROXY.)" >&2
    if echo "$_curl_log" | grep -q "CONNECT aborted"; then
      echo "hint: CONNECT reached board but the tunnel failed — check the \`pirate board\` terminal for lines like \`board <peer>: ...\` (ProxyTunnel, 403 block, session, etc.)." >&2
    fi
  fi
fi

echo
echo "=== 4) board stderr (manual) ==="
echo "Run \`pirate board ...\` in the foreground in another terminal. On each CONNECT,"
echo "check trace lines for direct TCP vs gRPC tunnel (see local-stack/client/src/board.rs trace_log)."

echo
echo "=== 5) server-side (manual, on deploy host) ==="
echo "While repeating step 2 or 3 from this machine, on the server run e.g.:"
echo "  sudo tcpdump -n -i any host api.ipify.org or port 8080"
echo "or inspect nginx / deploy-server logs for outbound connections sourced from the server."

echo
echo "Done."
