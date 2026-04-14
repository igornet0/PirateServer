#!/usr/bin/env bash
# Validates bypass (direct *.local), deny (403 block rule), proxy (tunnel to allowlisted host).
set -euo pipefail

BYPASS_OK=false
DENY_OK=false
PROXY_OK=false

if curl -sf --proxytunnel --proxy "http://${LISTEN_ADDR}" "http://bench.local:9000/" | grep -q "WIRE_UPSTREAM_OK"; then
  BYPASS_OK=true
fi

CODE="$(curl -sS -o /dev/null -w '%{http_code}' --proxytunnel --proxy "http://${LISTEN_ADDR}" "http://blocked.test:9000/" || true)"
if [[ "$CODE" == "403" ]]; then
  DENY_OK=true
fi

if curl -sf --proxytunnel --proxy "http://${LISTEN_ADDR}" "http://bench-upstream:9000/" | grep -q "WIRE_UPSTREAM_OK"; then
  PROXY_OK=true
fi

jq -n \
  --argjson bypass "$BYPASS_OK" \
  --argjson deny "$DENY_OK" \
  --argjson proxy "$PROXY_OK" \
  '{
    bypass_ok: $bypass,
    deny_ok: $deny,
    proxy_ok: $proxy
  }' >"${ROUTING_JSON_OUT:-/tmp/proxy-part-routing.json}"

if [[ "$BYPASS_OK" != true || "$DENY_OK" != true || "$PROXY_OK" != true ]]; then
  echo "routing: expected bypass+deny+proxy all true; got bypass=$BYPASS_OK deny=$DENY_OK proxy=$PROXY_OK" >&2
  exit 1
fi
