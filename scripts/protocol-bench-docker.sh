#!/usr/bin/env bash
# Run inside the protocol-bench container (tests/docker/docker-compose.protocol-bench.yml + run-protocol-bench.sh).
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-protocol-bench-config}"
CFG_DIR="${XDG_CONFIG_HOME}/pirate-client"
mkdir -p "$CFG_DIR"
SETTINGS="${CFG_DIR}/settings-protocol-bench.json"

BENCH_BYTES="${BENCH_BYTES:-33554432}"
BENCH_RUNS="${BENCH_RUNS:-3}"
BENCH_HOST="${BENCH_HOST:-wire-upstream}"
BENCH_PORT="${BENCH_PORT:-9000}"

VLESS_UUID="f47e3452-6ea8-4f3a-9c2b-1a0b9c8d7e6f"
TROJAN_PW="e2eTrojanSecret9"
VMESS_UUID="a1b2c3d4-e5f6-4a5b-8c9d-0e1f2a3b4c5d"

POLICY_JSON="$(jq -n \
  '{max_session_duration_sec: -1, traffic_total_bytes: -1, traffic_bytes_in_limit: -1, traffic_bytes_out_limit: -1, active_idle_timeout_sec: 300, never_expires: true}')"

bytes_to_mbps() {
  awk -v b="${1:-0}" 'BEGIN { printf "%.2f", (b * 8.0) / 1000000.0 }'
}

echo "==> protocol bench (Docker bridge; gRPC = h2c without TLS)"
echo "    BENCH_BYTES=${BENCH_BYTES} BENCH_RUNS=${BENCH_RUNS}"
echo ""

echo "==> health (control-api)"
curl -sf "$API/health" | grep -q ok

echo "==> install bundle + client auth"
BUNDLE="$(env DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=0 DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root /data print-install-bundle)"
if ! echo "$BUNDLE" | jq -e . >/dev/null 2>&1; then
  echo "expected JSON bundle on stdout" >&2
  exit 1
fi
client --endpoint "$GRPC" auth "$BUNDLE"
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
if [[ -z "$API_TOKEN" || "$API_TOKEN" == "null" ]]; then
  echo "login failed (need dashboard user + CONTROL_API_JWT_SECRET on control-api)" >&2
  exit 1
fi

create_session_plain() {
  local lab=$1
  curl -sf -X POST "${API}/api/v1/proxy-sessions?project=default" \
    -H "Authorization: Bearer ${API_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg lab "$lab" \
      --arg pk "$PUB" \
      --argjson pol "$POLICY_JSON" \
      '{"board_label":$lab,"policy":$pol,"recipient_client_pubkey_b64":$pk}')" \
    | jq -r '.session_token'
}

create_session_wire() {
  local lab=$1
  local wm=$2
  local cfg_json=$3
  curl -sf -X POST "${API}/api/v1/proxy-sessions?project=default" \
    -H "Authorization: Bearer ${API_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -n \
      --arg lab "$lab" \
      --argjson wm "$wm" \
      --argjson cfg "$cfg_json" \
      --arg pk "$PUB" \
      --argjson pol "$POLICY_JSON" \
      '{"board_label":$lab,"policy":$pol,"wire_mode":$wm,"wire_config":$cfg,"recipient_client_pubkey_b64":$pk}')" \
    | jq -r '.session_token'
}

median_tunnel_mbps() {
  local port=$1
  local token=$2
  local uri=$3
  if [[ -n "$uri" ]]; then
    jq -n --arg grpc "$GRPC" --arg uri "$uri" \
      '{default_board: "default", boards: {default: {enabled: true, url: $grpc, wire_subscription_uri: $uri}}}' >"$SETTINGS"
  else
    jq -n --arg grpc "$GRPC" \
      '{default_board: "default", boards: {default: {enabled: true, url: $grpc}}}' >"$SETTINGS"
  fi
  client board --listen "127.0.0.1:${port}" --endpoint "$GRPC" --session-token "$token" --settings "$SETTINGS" &
  local pid=$!
  sleep 5
  local -a speeds=()
  local i
  local sp
  for ((i = 0; i < BENCH_RUNS; i++)); do
    sp=$(curl -sS -o /dev/null -w '%{speed_download}' --max-time 600 -p \
      --proxy "http://127.0.0.1:${port}" \
      "http://${BENCH_HOST}:${BENCH_PORT}/size?bytes=${BENCH_BYTES}" || echo "0")
    speeds+=("$sp")
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  sleep 1
  local med
  med=$(printf '%s\n' "${speeds[@]}" | sort -n | awk '{a[NR]=$0} END {
    if (NR==0) print 0
    else if (NR%2==1) print a[(NR+1)/2]
    else print (a[NR/2]+a[NR/2+1])/2
  }')
  bytes_to_mbps "$med"
}

echo "==> create proxy sessions"
TOK_PLAIN="$(create_session_plain "bench-plain")"
CFG_VLESS="$(jq -n --arg u "$VLESS_UUID" '{uuid: $u}')"
TOK_VLESS="$(create_session_wire "bench-vless" 1 "$CFG_VLESS")"
CFG_TROJAN="$(jq -n --arg p "$TROJAN_PW" '{password: $p}')"
TOK_TROJAN="$(create_session_wire "bench-trojan" 2 "$CFG_TROJAN")"
CFG_VMESS="$(jq -n --arg u "$VMESS_UUID" '{uuid: $u}')"
TOK_VMESS="$(create_session_wire "bench-vmess" 3 "$CFG_VMESS")"

URI_VLESS="vless://${VLESS_UUID}@wire-upstream:9000?type=tcp"
URI_TROJAN="trojan://${TROJAN_PW}@wire-upstream:9000"
VMESS_JSON="$(jq -n \
  --arg add "wire-upstream" \
  --arg id "$VMESS_UUID" \
  '{v:"2", ps:"bench", add: $add, port: 9000, id: $id, aid: "0", net: "tcp", type: "none"}')"
VMESS_B64="$(printf '%s' "$VMESS_JSON" | base64 -w0 2>/dev/null || printf '%s' "$VMESS_JSON" | base64 | tr -d '\n')"
URI_VMESS="vmess://${VMESS_B64}"

echo "==> throughput: gRPC ConnectionProbe (max 4 MiB each way; JSON)"
GRPC_LINE=$(client board --test-connect --probe-json --endpoint "$GRPC" \
  --probe-upload-bytes 4194304 --probe-download-bytes 4194304 2>/dev/null || true)
if ! echo "$GRPC_LINE" | jq -e . >/dev/null 2>&1; then
  echo "FATAL: expected JSON from client board --probe-json" >&2
  exit 1
fi
G_RTT=$(echo "$GRPC_LINE" | jq -r '.get_status_rtt_ms')
G_UP=$(echo "$GRPC_LINE" | jq -r '.connection_probe_upload_mbps')
G_DN=$(echo "$GRPC_LINE" | jq -r '.connection_probe_download_mbps_est')

echo "==> throughput: ProxyTunnel (no wire) — download via HTTP CONNECT"
TP_PLAIN=$(median_tunnel_mbps 13140 "$TOK_PLAIN" "")

echo "==> throughput: VLESS"
TP_VLESS=$(median_tunnel_mbps 13128 "$TOK_VLESS" "$URI_VLESS")

echo "==> throughput: Trojan"
TP_TROJAN=$(median_tunnel_mbps 13129 "$TOK_TROJAN" "$URI_TROJAN")

echo "==> throughput: VMess"
TP_VMESS=$(median_tunnel_mbps 13130 "$TOK_VMESS" "$URI_VMESS")

echo ""
echo "================ THROUGHPUT (median download via tunnel; gRPC row = ConnectionProbe) ================"
printf "%-22s %10s %12s %12s %18s\n" "Protocol" "RTT_ms" "gRPC_up" "gRPC_down" "tunnel_DL_Mbps"
printf "%-22s %10s %12s %12s %18s\n" "----------------------" "----------" "------------" "------------" "------------------"
printf "%-22s %10s %12s %12s %18s\n" "gRPC_ConnectionProbe" "$(printf '%.2f' "$G_RTT")" "$(printf '%.2f' "$G_UP")" "$(printf '%.2f' "$G_DN")" "—"
printf "%-22s %10s %12s %12s %18s\n" "ProxyTunnel_plain" "—" "—" "—" "$TP_PLAIN"
printf "%-22s %10s %12s %12s %18s\n" "VLESS" "—" "—" "—" "$TP_VLESS"
printf "%-22s %10s %12s %12s %18s\n" "Trojan" "—" "—" "—" "$TP_TROJAN"
printf "%-22s %10s %12s %12s %18s\n" "VMess" "—" "—" "—" "$TP_VMESS"
echo ""

echo "==> security checks"
SEC_FAIL=0

echo -n "  grpc_GetStatus_without_auth ... "
set +e
GRPC_TRY=$(grpcurl -plaintext -import-path /proto -proto deploy.proto -d '{"project_id":"default"}' deploy-server:50051 deploy.DeployService/GetStatus 2>&1)
GRPC_RC=$?
set -e
if [[ "$GRPC_RC" -ne 0 ]] || echo "$GRPC_TRY" | grep -qiE 'Unauthenticated|unauthenticated|code = 16'; then
  echo "PASS"
  RES_GRPC=PASS
else
  echo "FAIL"
  RES_GRPC=FAIL
  SEC_FAIL=$((SEC_FAIL + 1))
fi

echo -n "  bad_session_token ... "
jq -n --arg grpc "$GRPC" '{default_board: "default", boards: {default: {enabled: true, url: $grpc}}}' >"$SETTINGS"
client board --listen "127.0.0.1:13300" --endpoint "$GRPC" --session-token "invalid-token-not-real" --settings "$SETTINGS" &
BPID=$!
sleep 5
set +e
curl -sS -o /dev/null --max-time 20 -p --proxy "http://127.0.0.1:13300" "http://${BENCH_HOST}:${BENCH_PORT}/size?bytes=1024"
CURL_BAD=$?
set -e
kill "$BPID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true
if [[ "$CURL_BAD" -ne 0 ]]; then
  echo "PASS"
  RES_BAD=PASS
else
  echo "FAIL"
  RES_BAD=FAIL
  SEC_FAIL=$((SEC_FAIL + 1))
fi

echo -n "  wrong_VLESS_secret_in_URI ... "
jq -n --arg grpc "$GRPC" --arg uri "vless://00000000-0000-0000-0000-000000000001@wire-upstream:9000?type=tcp" \
  '{default_board: "default", boards: {default: {enabled: true, url: $grpc, wire_subscription_uri: $uri}}}' >"$SETTINGS"
client board --listen "127.0.0.1:13301" --endpoint "$GRPC" --session-token "$TOK_VLESS" --settings "$SETTINGS" &
BPID=$!
sleep 5
set +e
curl -sS -o /dev/null --max-time 20 -p --proxy "http://127.0.0.1:13301" "http://${BENCH_HOST}:${BENCH_PORT}/"
CURL_W=$?
set -e
kill "$BPID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true
if [[ "$CURL_W" -ne 0 ]]; then
  echo "PASS"
  RES_WV=PASS
else
  echo "FAIL"
  RES_WV=FAIL
  SEC_FAIL=$((SEC_FAIL + 1))
fi

echo -n "  allowlist_deny_target ... "
jq -n --arg grpc "$GRPC" '{default_board: "default", boards: {default: {enabled: true, url: $grpc}}}' >"$SETTINGS"
client board --listen "127.0.0.1:13302" --endpoint "$GRPC" --session-token "$TOK_PLAIN" --settings "$SETTINGS" &
BPID=$!
sleep 5
set +e
curl -sS -o /dev/null --max-time 20 -p --proxy "http://127.0.0.1:13302" "http://deny.target:9000/"
CURL_D=$?
set -e
kill "$BPID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true
if [[ "$CURL_D" -ne 0 ]]; then
  echo "PASS"
  RES_DENY=PASS
else
  echo "FAIL"
  RES_DENY=FAIL
  SEC_FAIL=$((SEC_FAIL + 1))
fi

echo ""
echo "================ SECURITY (expected failures = PASS) ================"
printf "%-40s %s\n" "Check" "Result"
printf "%-40s %s\n" "----------------------------------------" "------"
printf "%-40s %s\n" "grpc_GetStatus_without_auth" "$RES_GRPC"
printf "%-40s %s\n" "bad_session_token" "$RES_BAD"
printf "%-40s %s\n" "wrong_VLESS_secret_in_URI" "$RES_WV"
printf "%-40s %s\n" "allowlist_deny_target" "$RES_DENY"
echo ""

echo "==> grpc-security-probe (replay nonce, bad metadata, ConnectionProbe chunk rules)"
if ! grpc-security-probe --endpoint "$GRPC"; then
  echo "grpc-security-probe failed" >&2
  SEC_FAIL=$((SEC_FAIL + 1))
fi

if [[ "$SEC_FAIL" -gt 0 ]]; then
  echo "protocol-bench: $SEC_FAIL security check(s) failed" >&2
  exit 1
fi

echo "OK: protocol bench passed"
