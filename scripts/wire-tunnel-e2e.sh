#!/usr/bin/env bash
# Run inside the wire-e2e container (see tests/docker/docker-compose.wire-e2e.yml + run-wire-tunnel-e2e.sh).
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-wire-e2e-config}"
CFG_DIR="${XDG_CONFIG_HOME}/pirate-client"
mkdir -p "$CFG_DIR"
SETTINGS="${CFG_DIR}/settings-wire-e2e.json"

VLESS_UUID="f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f"
TROJAN_PW="e2eTrojanSecret9"
VMESS_UUID="a1b2c3d4-e5f6-4a5b-8c9d-0e1f2a3b4c5d"

POLICY_JSON="$(jq -n \
  '{max_session_duration_sec: -1, traffic_total_bytes: -1, traffic_bytes_in_limit: -1, traffic_bytes_out_limit: -1, active_idle_timeout_sec: 300, never_expires: true}')"

echo "==> health (control-api)"
curl -sf "$API/health" | grep -q ok

echo "==> install bundle (read /data/.keys; stdout is one JSON line)"
BUNDLE="$(env DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=0 DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root /data print-install-bundle)"
if ! echo "$BUNDLE" | jq -e . >/dev/null 2>&1; then
  echo "expected JSON bundle on stdout, got: $BUNDLE" >&2
  exit 1
fi

echo "==> client auth"
client --endpoint "$GRPC" auth "$BUNDLE"

echo "==> client pubkey (recipient_client_pubkey_b64)"
PUB="$(client show-pubkey | tr -d '\n\r' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
if [[ -z "$PUB" ]]; then
  echo "empty pubkey" >&2
  exit 1
fi

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

create_session() {
  local label=$1
  local wm=$2
  local cfg_json=$3
  curl -sf -X POST "${API}/api/v1/proxy-sessions?project=default" \
    -H "Authorization: Bearer ${API_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg lab "$label" \
      --argjson wm "$wm" \
      --argjson cfg "$cfg_json" \
      --arg pk "$PUB" \
      --argjson pol "$POLICY_JSON" \
      '{"board_label":$lab,"policy":$pol,"wire_mode":$wm,"wire_config":$cfg,"recipient_client_pubkey_b64":$pk}')" \
    | jq -r '.session_token'
}

write_settings() {
  local uri=$1
  cat >"$SETTINGS" <<EOF
{
  "default_board": "default",
  "boards": {
    "default": {
      "enabled": true,
      "url": "$GRPC",
      "wire_subscription_uri": "$uri"
    }
  }
}
EOF
}

run_one() {
  local name=$1 port=$2 token=$3 uri=$4
  write_settings "$uri"
  echo "==> direct reachability to wire-upstream:9000 ($name)"
  if ! curl -sS --max-time 10 "http://wire-upstream:9000/" | grep -q WIRE_UPSTREAM_OK; then
    echo "direct GET http://wire-upstream:9000/ did not return WIRE_UPSTREAM_OK" >&2
    exit 1
  fi
  echo "==> board + curl via ProxyTunnel ($name)"
  client board \
    --listen "127.0.0.1:${port}" \
    --endpoint "$GRPC" \
    --session-token "$token" \
    --settings "$SETTINGS" &
  local pid=$!
  sleep 5
  # HTTP proxy must use CONNECT (board is CONNECT-only); plain http:// via -x defaults to absolute-URI, not tunnel.
  local resp
  if ! resp=$(curl -sS --max-time 60 -p --proxy "http://127.0.0.1:${port}" "http://wire-upstream:9000/" 2>&1); then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    echo "curl via proxy failed for $name: $resp" >&2
    exit 1
  fi
  if ! echo "$resp" | grep -q WIRE_UPSTREAM_OK; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    echo "unexpected body for $name (want WIRE_UPSTREAM_OK): $resp" >&2
    exit 1
  fi
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

echo "==> VLESS session + tunnel"
CFG_VLESS="$(jq -n --arg u "$VLESS_UUID" '{uuid: $u}')"
TOK_VLESS="$(create_session "e2e-vless" 1 "$CFG_VLESS")"
URI_VLESS="vless://${VLESS_UUID}@wire-upstream:9000?type=tcp"
run_one "VLESS" 13128 "$TOK_VLESS" "$URI_VLESS"

echo "==> Trojan session + tunnel"
CFG_TROJAN="$(jq -n --arg p "$TROJAN_PW" '{password: $p}')"
TOK_TROJAN="$(create_session "e2e-trojan" 2 "$CFG_TROJAN")"
URI_TROJAN="trojan://${TROJAN_PW}@wire-upstream:9000"
run_one "Trojan" 13129 "$TOK_TROJAN" "$URI_TROJAN"

echo "==> VMess session + tunnel"
CFG_VMESS="$(jq -n --arg u "$VMESS_UUID" '{uuid: $u}')"
TOK_VMESS="$(create_session "e2e-vmess" 3 "$CFG_VMESS")"
VMESS_JSON="$(jq -n \
  --arg add "wire-upstream" \
  --arg id "$VMESS_UUID" \
  '{v:"2", ps:"e2e", add: $add, port: 9000, id: $id, aid: "0", net: "tcp", type: "none"}')"
VMESS_B64="$(printf '%s' "$VMESS_JSON" | base64 -w0 2>/dev/null || printf '%s' "$VMESS_JSON" | base64 | tr -d '\n')"
URI_VMESS="vmess://${VMESS_B64}"
run_one "VMess" 13130 "$TOK_VMESS" "$URI_VMESS"

echo "OK: wire tunnel e2e passed"
