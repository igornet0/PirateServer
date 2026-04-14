#!/usr/bin/env bash
# Load + latency (p50/p95/p99) + optional Prometheus scrape — run inside protocol-load container.
set -euo pipefail

API="${CONTROL_API_DIRECT:-http://control-api:8080}"
GRPC="${GRPC_ENDPOINT:-http://deploy-server:50051}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-/tmp/pirate-protocol-load-config}"
CFG_DIR="${XDG_CONFIG_HOME}/pirate-client"
mkdir -p "$CFG_DIR"

LATENCY_SAMPLES="${LATENCY_SAMPLES:-200}"
LOAD_RPS_DURATION_SEC="${LOAD_RPS_DURATION_SEC:-5}"
LOAD_PARALLEL="${LOAD_PARALLEL:-8}"
METRICS_URL="${METRICS_URL:-http://deploy-server:9090}"

echo "==> protocol-load (LATENCY_SAMPLES=$LATENCY_SAMPLES LOAD_RPS_DURATION_SEC=$LOAD_RPS_DURATION_SEC LOAD_PARALLEL=$LOAD_PARALLEL)"

echo "==> health (control-api)"
curl -sf "$API/health" | grep -q ok

echo "==> install bundle + client auth"
BUNDLE="$(env DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=0 DEPLOY_GRPC_PUBLIC_URL="$GRPC" deploy-server --root /data print-install-bundle)"
if ! echo "$BUNDLE" | jq -e . >/dev/null 2>&1; then
  echo "expected JSON bundle on stdout" >&2
  exit 1
fi
client --endpoint "$GRPC" auth "$BUNDLE"

echo "==> latency samples (client status, ms)"
rm -f /tmp/lat.txt
touch /tmp/lat.txt
for _ in $(seq 1 "$LATENCY_SAMPLES"); do
  t0=$(date +%s%N)
  client --endpoint "$GRPC" status >/dev/null 2>&1 || true
  t1=$(date +%s%N)
  echo $(( (t1 - t0) / 1000000 )) >>/tmp/lat.txt
done

sort -n /tmp/lat.txt -o /tmp/lat_sorted.txt
NL=$(wc -l </tmp/lat_sorted.txt | tr -d ' ')
idx50=$(( (NL * 50 + 99) / 100 ))
idx95=$(( (NL * 95 + 99) / 100 ))
idx99=$(( (NL * 99 + 99) / 100 ))
[[ "$idx50" -lt 1 ]] && idx50=1
[[ "$idx95" -lt 1 ]] && idx95=1
[[ "$idx99" -lt 1 ]] && idx99=1
P50=$(sed -n "${idx50}p" /tmp/lat_sorted.txt)
P95=$(sed -n "${idx95}p" /tmp/lat_sorted.txt)
P99=$(sed -n "${idx99}p" /tmp/lat_sorted.txt)

echo "================ LATENCY (ms) ================"
echo "samples=$NL p50=$P50 p95=$P95 p99=$P99"
echo ""

echo "==> RPS burst (parallel status, duration ${LOAD_RPS_DURATION_SEC}s)"
start=$(date +%s)
n=0
while true; do
  now=$(date +%s)
  if [[ $((now - start)) -ge "$LOAD_RPS_DURATION_SEC" ]]; then
    break
  fi
  for _ in $(seq 1 "$LOAD_PARALLEL"); do
    client --endpoint "$GRPC" status >/dev/null 2>&1 && n=$((n + 1)) &
  done
  wait
done
echo "approx_requests=$n duration_sec=$LOAD_RPS_DURATION_SEC parallel=$LOAD_PARALLEL"
echo ""

echo "==> metrics (deploy_proxy_*, if DEPLOY_METRICS_BIND set)"
if curl -sf "$METRICS_URL/metrics" >/tmp/metrics.txt 2>/dev/null; then
  grep -E '^deploy_proxy|^# HELP deploy_proxy' /tmp/metrics.txt | head -20 || true
else
  echo "(metrics not reachable at $METRICS_URL — check tests/docker/docker-compose.protocol-ext.yml merge)"
fi

echo "OK: protocol-load finished"
