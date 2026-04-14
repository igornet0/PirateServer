#!/usr/bin/env bash
# Orchestrator (runs inside proxy-test container as root for optional tc).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$SCRIPT_DIR/proxy_test_common.sh"

REPORT_PATH="${PROXY_TEST_REPORT_PATH:-/artifacts/proxy-test-report.json}"
mkdir -p "$(dirname "$REPORT_PATH")"
mkdir -p /artifacts

SUITE="${PROXY_TEST_SUITE:-basic}"

if [[ "${TEST_MODE:-local}" == "remote" ]]; then
  export GRPC_ENDPOINT="${REMOTE_GRPC_ENDPOINT:-${GRPC_ENDPOINT:-}}"
  export CONTROL_API_DIRECT="${REMOTE_CONTROL_API:-${CONTROL_API_DIRECT:-}}"
fi

MODE=synthetic
if [[ -n "${NETEM_BANDWIDTH:-}" || "${NETEM_DELAY_MS:-0}" != "0" || "${NETEM_LOSS_PERCENT:-0}" != "0" ]]; then
  MODE=simulated
fi
if [[ "${TEST_MODE:-local}" == "remote" ]]; then
  MODE=real
fi

echo "==> proxy test suite (suite=${SUITE} mode=${MODE})"
if [[ "$SUITE" == "network" && "$MODE" == "synthetic" ]]; then
  echo "warning: suite=network but NETEM_* not set — metrics stay synthetic (loopback bridge)." >&2
fi
bash "$SCRIPT_DIR/network_setup.sh"

proxy_test_health
echo "==> control-api JWT (login)"
proxy_test_login
echo "==> install bundle + client auth"
proxy_test_install_and_auth

echo "==> gRPC connection probe (before local board)"
PROBE_JSON="$(
  client board --test-connect --probe-json --endpoint "$GRPC" \
    --probe-upload-bytes 2097152 --probe-download-bytes 2097152 2>/dev/null || echo '{}'
)"
echo "$PROBE_JSON" | jq -e . >/dev/null 2>&1 || PROBE_JSON='{}'

echo "==> create proxy session + start board"
proxy_test_create_session
proxy_test_start_board

rm -f /tmp/proxy-part-*.json

run_phase() {
  case "$SUITE" in
    basic)
      bash "$SCRIPT_DIR/run_routing_test.sh"
      bash "$SCRIPT_DIR/run_bandwidth_test.sh"
      ;;
    network)
      bash "$SCRIPT_DIR/run_routing_test.sh"
      bash "$SCRIPT_DIR/run_bandwidth_test.sh"
      ;;
    load)
      bash "$SCRIPT_DIR/run_stability_test.sh"
      bash "$SCRIPT_DIR/run_concurrency_test.sh"
      ;;
    full | all)
      bash "$SCRIPT_DIR/run_routing_test.sh"
      bash "$SCRIPT_DIR/run_bandwidth_test.sh"
      bash "$SCRIPT_DIR/run_stability_test.sh"
      bash "$SCRIPT_DIR/run_concurrency_test.sh"
      if [[ "${RUN_FAILURE_TESTS:-0}" == "1" ]]; then
        bash "$SCRIPT_DIR/run_failure_scenarios.sh"
      fi
      ;;
    *)
      echo "unknown PROXY_TEST_SUITE=$SUITE" >&2
      exit 1
      ;;
  esac
}

run_phase

FAILURE_MERGE='{}'
if [[ -f /tmp/proxy-part-failure.json ]]; then
  FAILURE_MERGE="$(cat /tmp/proxy-part-failure.json)"
else
  FAILURE_MERGE="$(jq -n \
    --arg s "$(if [[ "${RUN_FAILURE_TESTS:-0}" == "1" ]]; then echo "missing"; else echo "skipped"; fi)" \
    '{skipped:true, reason:$s}')"
fi

ROUTING_MERGE='{}'
[[ -f /tmp/proxy-part-routing.json ]] && ROUTING_MERGE="$(cat /tmp/proxy-part-routing.json)"

BW_MERGE='{}'
[[ -f /tmp/proxy-part-bandwidth.json ]] && BW_MERGE="$(cat /tmp/proxy-part-bandwidth.json)"

STAB_MERGE='{}'
[[ -f /tmp/proxy-part-stability.json ]] && STAB_MERGE="$(cat /tmp/proxy-part-stability.json)"

CONC_MERGE='{}'
[[ -f /tmp/proxy-part-concurrency.json ]] && CONC_MERGE="$(cat /tmp/proxy-part-concurrency.json)"

RTT="$(echo "$PROBE_JSON" | jq -r '.get_status_rtt_ms // empty')"
DL_AVG="$(echo "$BW_MERGE" | jq -r '.download_mbps.avg // empty')"
UL_AVG="$(echo "$BW_MERGE" | jq -r '.upload_mbps.avg // empty')"
DL_P95="$(echo "$BW_MERGE" | jq -r '.download_mbps.p95 // empty')"

jq -n \
  --arg mode "$MODE" \
  --arg suite "$SUITE" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson probe "$PROBE_JSON" \
  --argjson routing "$ROUTING_MERGE" \
  --argjson bandwidth "$BW_MERGE" \
  --argjson stability "$STAB_MERGE" \
  --argjson concurrency "$CONC_MERGE" \
  --argjson failure "$FAILURE_MERGE" \
  --argjson netem "$(jq -n \
    --arg d "${NETEM_DELAY_MS:-0}" \
    --arg l "${NETEM_LOSS_PERCENT:-0}" \
    --arg b "${NETEM_BANDWIDTH:-}" \
    '{delay_ms:($d|tonumber),loss_percent:($l|tonumber),bandwidth: (if $b=="" then null else $b end)}')" \
  '{
    generated_at: $ts,
    mode: $mode,
    suite: $suite,
    connection: {probe: $probe},
    netem: $netem,
    routing: $routing,
    bandwidth: $bandwidth,
    stability: $stability,
    concurrency: $concurrency,
    failure_scenarios: $failure
  }' >"$REPORT_PATH"

echo ""
echo "=== PROXY TEST REPORT ==="
echo ""
echo "[meta]"
echo "mode: $MODE"
echo "suite: $SUITE"
echo "report: $REPORT_PATH"
echo ""
echo "[connection]"
echo "status: OK"
if [[ -n "$RTT" ]]; then
  echo "rtt_ms: $RTT"
fi
echo ""
if [[ "$BW_MERGE" != "{}" ]]; then
  echo "[bandwidth]"
  echo "download_avg_mbps: ${DL_AVG:-n/a}"
  echo "download_p95_mbps: ${DL_P95:-n/a}"
  echo "upload_avg_mbps: ${UL_AVG:-n/a}"
  echo ""
fi
if [[ "$STAB_MERGE" != "{}" ]]; then
  echo "[stability]"
  echo "$STAB_MERGE" | jq -r '"duration_sec: \(.duration_sec)\ndisconnects: \(.disconnects)"'
  echo ""
fi
if [[ "$CONC_MERGE" != "{}" ]]; then
  echo "[concurrency]"
  echo "$CONC_MERGE" | jq -r '"connections: \(.connections)\nsuccess_rate: \(.success_rate)\navg_latency_s: \(.avg_latency_s)"'
  echo ""
fi
if [[ "$ROUTING_MERGE" != "{}" ]]; then
  echo "[routing]"
  echo "$ROUTING_MERGE" | jq -r '"bypass: \(if .bypass_ok then "OK" else "FAIL" end)\ndeny: \(if .deny_ok then "OK" else "FAIL" end)\nproxy: \(if .proxy_ok then "OK" else "FAIL" end)"'
  echo ""
fi
if [[ "${RUN_FAILURE_TESTS:-0}" == "1" ]] && [[ -f /tmp/proxy-part-failure.json ]]; then
  echo "[failure_scenarios]"
  cat /tmp/proxy-part-failure.json | jq .
  echo ""
fi

echo "OK: proxy test suite finished"
