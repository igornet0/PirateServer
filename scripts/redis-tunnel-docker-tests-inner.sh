#!/usr/bin/env bash
# Runs inside redis-tunnel-runner (pirate-test-runtime). Requires BUNDLE_JSON from host.
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-redis-tunnel-test}"
CFG_DIR="$XDG_CONFIG_HOME/pirate-client"
mkdir -p "$CFG_DIR"

if [[ -z "${BUNDLE_JSON:-}" ]]; then
  echo "redis-tunnel-docker-tests-inner: BUNDLE_JSON is not set" >&2
  exit 1
fi

echo "==> client auth (install bundle)"
echo "$BUNDLE_JSON" | client --endpoint "$GRPC" auth

PUB="$(client show-pubkey | tr -d '\n\r' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
if [[ -z "$PUB" ]]; then
  echo "empty pubkey after auth" >&2
  exit 1
fi

POLICY_JSON="$(jq -n \
  '{max_session_duration_sec: -1, traffic_total_bytes: -1, traffic_bytes_in_limit: -1, traffic_bytes_out_limit: -1, active_idle_timeout_sec: 300, never_expires: true}')"

echo "==> control-api JWT (login)"
API_TOKEN="$(
  curl -sf -X POST "${API}/api/v1/auth/login" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg u "${CONTROL_API_LOGIN_USER:-admin}" \
      --arg p "${CONTROL_API_LOGIN_PASSWORD:-testpass}" \
      '{username:$u,password:$p}')" \
  | jq -r '.access_token'
)"
[[ -n "$API_TOKEN" && "$API_TOKEN" != "null" ]]

echo "==> create managed proxy session (control-api → deploy-server)"
TOK="$(curl -sf -X POST "${API}/api/v1/proxy-sessions?project=default" \
  -H "Authorization: Bearer ${API_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "$(jq -n \
    --arg lab "redis-docker-test" \
    --arg pk "$PUB" \
    --argjson pol "$POLICY_JSON" \
    '{"board_label":$lab,"policy":$pol,"recipient_client_pubkey_b64":$pk}')" \
  | jq -r '.session_token')"

if [[ -z "$TOK" || "$TOK" == "null" ]]; then
  echo "failed to obtain session_token" >&2
  exit 1
fi

echo "==> board + one proxied GET (ProxyTunnel → bench-upstream)"
PORT=18788
jq -n --arg grpc "$GRPC" \
  '{default_board: "default", boards: {default: {enabled: true, url: $grpc}}}' >"$CFG_DIR/settings.json"

client board --listen "127.0.0.1:${PORT}" --endpoint "$GRPC" --session-token "$TOK" --settings "$CFG_DIR/settings.json" &
BPID=$!
sleep 5

curl -sfS -o /dev/null --max-time 60 -p \
  --proxy "http://127.0.0.1:${PORT}" \
  "http://bench-upstream:9000/size?bytes=8192"

kill "$BPID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true
sleep 2

echo "OK: inner tunnel smoke finished"
