#!/usr/bin/env bash
set -euo pipefail

BASE="${LISTENER_HTTP:-http://listener:9380}"
BASE="${BASE%/}"
GRPC="${LISTENER_GRPC:-${BASE/:9380/:9381}}"
TOK="${STACK_TUN_REST_BEARER:?need STACK_TUN_REST_BEARER}"
ID_FILE="${IDENTITY_JSON:-/tmp/e2e/identity.json}"
PROFILE_ID="e2e-bus-listen"
HDR_AUTH=(-H "Authorization: Bearer ${TOK}")

auth_curl() { curl -fsS "$@"; }

make_identity() {
  mkdir -p "$(dirname "$ID_FILE")"
  if [[ ! -s "$ID_FILE" ]]; then
    local seed
    seed="$(openssl rand -base64 32 | tr -d '\n')"
    printf '{"private_key_b64":"%s"}\n' "$seed" >"$ID_FILE"
  fi
}

worker_pubkey_b64() {
  pirate show-pubkey --identity "$ID_FILE"
}

# Long‑running helpers must NOT run under `$(…)` capture: Bash waits for background jobs spawned
# inside command substitution subshells to finish before assigning the PID, which deadlocks forever.
UPSTREAM_PID=0
TUNNEL_PID=0

upstream_start() {
  mkdir -p /srv/e2e
  echo "${1:?marker}" >/srv/e2e/index.html
  python3 -m http.server 8080 --bind 127.0.0.1 --directory /srv/e2e &
  UPSTREAM_PID=$!
}

tunnel_start() {
  local mode="$1"
  pirate tunnel \
    --url "$BASE" \
    --bearer "$TOK" \
    --grpc "$GRPC" \
    --listen-profile-id "$PROFILE_ID" \
    --target "127.0.0.1:8080" \
    --identity "$ID_FILE" \
    --pull-wait-ms 5000 \
    --mode "${mode}" &
  TUNNEL_PID=$!
}

tunnel_stop() { kill "$1" 2>/dev/null || true; wait "$1" 2>/dev/null || true; }

upstream_stop() { kill "$1" 2>/dev/null || true; wait "$1" 2>/dev/null || true; }

authorize_peer() {
  local pk="$1"
  auth_curl "${HDR_AUTH[@]}" -X POST "${BASE}/api/v1/peers" \
    -H "Content-Type: application/json" \
    --data-binary "{\"publicKeyB64\":\"${pk}\"}"
}

submit_task() {
  auth_curl "${HDR_AUTH[@]}" -X POST "${BASE}/api/v1/tasks" \
    -H "Content-Type: application/json" \
    --data-binary @- <<EOF
{"profileId":"${PROFILE_ID}","method":"GET","scheme":"http","host":"svc","path":"/","headers":{}}
EOF
}

wait_completed() {
  local rid="$1"
  local i
  for i in $(seq 1 90); do
    local snap
    snap="$(auth_curl "${HDR_AUTH[@]}" "${BASE}/api/v1/tasks/${rid}?profileId=${PROFILE_ID}")"
    local ph
    ph="$(echo "$snap" | jq -r '.phase')"
    if [[ "$ph" == "completed" ]]; then
      echo "$snap"
      return 0
    fi
    if [[ "$ph" == "failed" || "$ph" == "expired" ]]; then
      echo "$snap" >&2
      return 1
    fi
    sleep 0.3
  done
  return 1
}

run_one_case() {
  local mode="$1"
  local marker="$2"
  echo "===== case: tunnel --mode ${mode}, marker=${marker} ====="

  local up_pid tn_pid pk sub rid snap body
  upstream_start "${marker}"
  up_pid="$UPSTREAM_PID"

  sleep 0.3

  make_identity
  pk="$(worker_pubkey_b64)"

  tunnel_start "${mode}"
  tn_pid="$TUNNEL_PID"
  sleep 1

  sub="$(submit_task)"
  rid="$(echo "$sub" | jq -r '.requestId')"
  if [[ -z "$rid" || "$rid" == "null" ]]; then
    echo "submit failed: $sub" >&2
    return 1
  fi

  snap="$(wait_completed "$rid")"
  body="$(echo "$snap" | jq -r '.bodyBase64' | base64 -d)"
  tunnel_stop "${tn_pid}"
  upstream_stop "${up_pid}"

  if [[ "$body" != *"${marker}"* ]]; then
    echo "BAD body (expected substring ${marker}): $body" >&2
    return 1
  fi
  echo "OK ${mode}: task ${rid}"
}

reset_listen_profile_public_fields() {
  # Back to trusting any authorized peer (empty allow-list) for repeatable --mode local first.
  local cfg out
  cfg="$(auth_curl "${HDR_AUTH[@]}" "${BASE}/api/v1/config")"
  out="$(echo "$cfg" | jq '
    (.profiles |= map(if .id == "e2e-bus-listen" then
      .connectorAllowPubkeyB64 = [] | .linkKind = "local"
    else . end))
  | {profiles: .profiles}')"
  auth_curl "${HDR_AUTH[@]}" -X PUT "${BASE}/api/v1/config" \
    -H "Content-Type: application/json" \
    --data-binary "$out"
}

main() {
  auth_curl http://listener:9380/health >/dev/null

  make_identity
  authorize_peer "$(worker_pubkey_b64)"

  reset_listen_profile_public_fields

  run_one_case "local" "E2E-LOCAL-TUNNEL-OK"

  reset_listen_profile_public_fields
  run_one_case "public" "E2E-PUBLIC-TUNNEL-OK"

  echo "ALL stack-tun task-queue docker tests passed."
}

main "$@"
