#!/usr/bin/env bash
# Long-lived chunked download through proxy; counts hard failures as disconnects.
set -euo pipefail

STABILITY_SEC="${STABILITY_SEC:-30}"
DISCONNECTS=0

# Single long stream (bench /stream endpoint).
if ! curl -sf --proxytunnel --proxy "http://${LISTEN_ADDR}" \
  --max-time "$((STABILITY_SEC + 15))" \
  -o /dev/null \
  "http://bench-upstream:9000/stream?seconds=${STABILITY_SEC}"; then
  DISCONNECTS=$((DISCONNECTS + 1))
fi

jq -n \
  --argjson d "$DISCONNECTS" \
  --argjson sec "$STABILITY_SEC" \
  '{duration_sec: $sec, disconnects: $d}' >"${STABILITY_JSON_OUT:-/tmp/proxy-part-stability.json}"

if [[ "$DISCONNECTS" -gt 0 ]]; then
  echo "stability: disconnects=$DISCONNECTS" >&2
  exit 1
fi
