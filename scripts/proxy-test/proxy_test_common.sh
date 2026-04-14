#!/usr/bin/env bash
# Shared bootstrap for proxy E2E (auth, JWT, session, settings with routing rules, board process).
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-proxy-test-config}"
CFG_DIR="${XDG_CONFIG_HOME}/pirate-client"
mkdir -p "$CFG_DIR"
RULES_DIR="${PROXY_TEST_RULES_DIR:-/tmp/proxy-test-rules}"
mkdir -p "$RULES_DIR"
BLOCK_JSON="${RULES_DIR}/block.json"
SETTINGS="${CFG_DIR}/settings-proxy-test.json"
LISTEN_PORT="${PROXY_TEST_LISTEN_PORT:-13280}"
LISTEN_ADDR="127.0.0.1:${LISTEN_PORT}"
POLICY_JSON='{"max_session_duration_sec":-1,"traffic_total_bytes":-1,"traffic_bytes_in_limit":-1,"traffic_bytes_out_limit":-1,"active_idle_timeout_sec":300,"never_expires":true}'

BOARD_PID=""

proxy_test_write_block_rules() {
  cat >"$BLOCK_JSON" <<'EOF'
{"version":1,"last_updated":"2026-01-01","domains_block":["blocked.test"],"domain_patterns_block":[],"ips_block":[]}
EOF
}

proxy_test_write_settings() {
  proxy_test_write_block_rules
  jq -n \
    --arg grpc "$GRPC" \
    --arg bj "$BLOCK_JSON" \
    --arg tm "${PIRATE_TRANSPORT_MODE:-auto}" \
    '{
      version: 1,
      global: {
        bypass: ["*.local"],
        traffic_rule_source: "merged",
        default_rules: {block_json: $bj}
      },
      default_board: "default",
      boards: {
        default: {enabled: true, url: $grpc, transport_mode: $tm, quic_tls_insecure: true}
      }
    }' >"$SETTINGS"
}

proxy_test_health() {
  curl -sf "$API/health" | grep -q ok
}

proxy_test_login() {
  local LOGIN_JSON HTTP
  LOGIN_JSON="$(mktemp)"
  HTTP="$(
    curl -sS -X POST "${API}/api/v1/auth/login" \
      -H "Content-Type: application/json" \
      -d "$(jq -n \
        --arg u "${CONTROL_API_LOGIN_USER:-admin}" \
        --arg p "${CONTROL_API_LOGIN_PASSWORD:-testpass}" \
        '{username:$u,password:$p}')" \
      -o "$LOGIN_JSON" -w "%{http_code}"
  )"
  if [[ "$HTTP" != "200" ]]; then
    echo "login failed HTTP ${HTTP}: $(cat "$LOGIN_JSON")" >&2
    rm -f "$LOGIN_JSON"
    return 1
  fi
  API_TOKEN="$(jq -r '.access_token' "$LOGIN_JSON")"
  rm -f "$LOGIN_JSON"
  API_TOKEN="${API_TOKEN//$'\r'/}"
  [[ -n "$API_TOKEN" && "$API_TOKEN" != "null" ]]
  export API_TOKEN
}

proxy_test_install_and_auth() {
  local BUNDLE PUB
  BUNDLE="$(env DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=0 DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root /data print-install-bundle)"
  echo "$BUNDLE" | jq -e . >/dev/null
  client --endpoint "$GRPC" auth "$BUNDLE"
  PUB="$(client show-pubkey | tr -d '\n\r' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  [[ -n "$PUB" ]]
  export PUB
}

proxy_test_create_session() {
  local SESS_JSON HTTP
  SESS_JSON="$(mktemp)"
  SESS_BODY="$(jq -n \
    --arg board_label "proxy-test-suite" \
    --arg pk "$PUB" \
    --argjson pol "$POLICY_JSON" \
    '{"board_label":$board_label,"policy":$pol,"recipient_client_pubkey_b64":$pk}')"
  HTTP="$(
    curl -sS -X POST "${API}/api/v1/proxy-sessions?project=default" \
      -H "Authorization: Bearer ${API_TOKEN}" \
      -H "Content-Type: application/json" \
      -d "$SESS_BODY" \
      -o "$SESS_JSON" -w "%{http_code}"
  )"
  if [[ "$HTTP" != "200" ]]; then
    echo "POST /api/v1/proxy-sessions failed HTTP ${HTTP}: $(cat "$SESS_JSON")" >&2
    rm -f "$SESS_JSON"
    return 1
  fi
  SESSION_TOKEN="$(jq -r '.session_token' "$SESS_JSON")"
  rm -f "$SESS_JSON"
  [[ -n "$SESSION_TOKEN" && "$SESSION_TOKEN" != "null" ]]
  export SESSION_TOKEN
}

proxy_test_grpc_probe() {
  client board --test-connect --probe-json --endpoint "$GRPC" \
    --probe-upload-bytes 2097152 --probe-download-bytes 2097152 2>/dev/null || true
}

proxy_test_start_board() {
  proxy_test_write_settings
  client board \
    --listen "$LISTEN_ADDR" \
    --endpoint "$GRPC" \
    --session-token "$SESSION_TOKEN" \
    --settings "$SETTINGS" &
  BOARD_PID=$!
  export BOARD_PID
  trap 'kill "$BOARD_PID" 2>/dev/null || true; wait "$BOARD_PID" 2>/dev/null || true' EXIT
  sleep 4
}

proxy_test_stop_board() {
  if [[ -n "${BOARD_PID:-}" ]] && kill -0 "$BOARD_PID" 2>/dev/null; then
    kill "$BOARD_PID" 2>/dev/null || true
    wait "$BOARD_PID" 2>/dev/null || true
  fi
  BOARD_PID=""
  trap - EXIT
}
