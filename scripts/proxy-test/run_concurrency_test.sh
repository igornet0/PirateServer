#!/usr/bin/env bash
# Parallel downloads; success rate + average request time (per-parallel sample).
set -euo pipefail

N="${CONCURRENCY_CONNECTIONS:-10}"
BYTES="${CONCURRENCY_BYTES:-262144}"
MAX_N="${MAX_CONCURRENCY_CONNECTIONS:-50}"
if [[ "$N" -gt "$MAX_N" ]]; then
  echo "concurrency: capping connections $N -> $MAX_N" >&2
  N="$MAX_N"
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
declare -a pids=()
fail=0
for ((i = 0; i < N; i++)); do
  (
    curl -sf --proxytunnel --proxy "http://${LISTEN_ADDR}" \
      -o /dev/null -w '%{time_total}\n' \
      "http://bench-upstream:9000/size?bytes=${BYTES}" \
      >"$tmpdir/t$i" || exit 1
  ) &
  pids+=($!)
done
for pid in "${pids[@]}"; do
  if ! wait "$pid"; then
    fail=$((fail + 1))
  fi
done

SUCCESS_RATE="$(awk -v f="$fail" -v n="$N" 'BEGIN{if(n<=0){print 0;exit} printf "%.6f",(n-f)/n}')"
AVG_LAT="$(awk '{s+=$1;c++} END{if(c>0) printf "%.6f", s/c; else print "0"}' "$tmpdir"/t* 2>/dev/null || echo "0")"

jq -n \
  --argjson n "$N" \
  --argjson fail "$fail" \
  --argjson sr "$SUCCESS_RATE" \
  --argjson avgl "$AVG_LAT" \
  '{
    connections: $n,
    failures: $fail,
    success_rate: $sr,
    avg_latency_s: $avgl
  }' >"${CONCURRENCY_JSON_OUT:-/tmp/proxy-part-concurrency.json}"

MIN_SR="${MIN_SUCCESS_RATE:-0.95}"
awk -v sr="$SUCCESS_RATE" -v m="$MIN_SR" 'BEGIN{exit (sr+0 >= m+0) ? 0 : 1}' || {
  echo "concurrency: success_rate ${SUCCESS_RATE} < MIN_SUCCESS_RATE ${MIN_SR}" >&2
  exit 1
}

MAX_FAIL="${MAX_CONNECTION_FAILURE_RATE:-0.05}"
if [[ "$N" -le 0 ]]; then
  FAIL_RATE="1"
else
  FAIL_RATE="$(awk -v f="$fail" -v n="$N" 'BEGIN{printf "%.6f", f/n}')"
fi
awk -v fr="$FAIL_RATE" -v m="$MAX_FAIL" 'BEGIN{exit (fr<=m)?0:1}' || {
  echo "concurrency: failure_rate ${FAIL_RATE} > MAX_CONNECTION_FAILURE_RATE ${MAX_FAIL}" >&2
  exit 1
}
