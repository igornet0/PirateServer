#!/usr/bin/env bash
# In-container negative tests: blocked host (403), dead upstream (graceful), board stays up.
# Host-only chaos (control-api down / deploy restart) is out of scope here — reported as skipped.
set -euo pipefail

DEAD_PORT="${FAILURE_DEAD_PORT:-9}"
BOARD_PID="${BOARD_PID:-}"

blocked_is_403() {
  local c
  c="$(curl -sS -o /dev/null -w '%{http_code}' --proxytunnel --proxy "http://${LISTEN_ADDR}" "http://blocked.test:9000/" || true)"
  [[ "$c" == "403" ]]
}

dead_upstream_graceful() {
  # Connection to reserved port on localhost should fail; proxy must not kill the board.
  ! curl -sf --max-time 2 --proxytunnel --proxy "http://${LISTEN_ADDR}" \
    "http://127.0.0.1:${DEAD_PORT}/" -o /dev/null
}

board_alive() {
  [[ -n "$BOARD_PID" ]] && kill -0 "$BOARD_PID" 2>/dev/null
}

BLOCK_OK=false
DEAD_OK=false
ALIVE_OK=false

blocked_is_403 && BLOCK_OK=true
dead_upstream_graceful && DEAD_OK=true
board_alive && ALIVE_OK=true

jq -n \
  --argjson block "$BLOCK_OK" \
  --argjson dead "$DEAD_OK" \
  --argjson alive "$ALIVE_OK" \
  --arg skip "host_chaos_skipped" \
  '{
    blocked_returns_403: $block,
    dead_upstream_no_crash: $dead,
    board_still_running: $alive,
    note: $skip
  }' >"${FAILURE_JSON_OUT:-/tmp/proxy-part-failure.json}"

if [[ "$BLOCK_OK" != true || "$DEAD_OK" != true || "$ALIVE_OK" != true ]]; then
  echo "failure_scenarios: block=$BLOCK_OK dead_ok=$DEAD_OK alive=$ALIVE_OK" >&2
  exit 1
fi
