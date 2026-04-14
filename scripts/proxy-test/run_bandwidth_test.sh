#!/usr/bin/env bash
# Time-based bandwidth through CONNECT proxy; multiple iterations; median + p95 Mbps.
# Expects: LISTEN_ADDR, bench-upstream reachable, board running.
set -euo pipefail

RUNS="${PROXY_TEST_RUNS:-3}"
BYTES="${PROXY_TEST_BW_BYTES:-2097152}"

run_download_once() {
  curl -sS --proxytunnel --proxy "http://${LISTEN_ADDR}" \
    -o /dev/null -w '%{time_total}' \
    "http://bench-upstream:9000/size?bytes=${BYTES}"
}

run_upload_once() {
  local tmp
  tmp="$(mktemp)"
  dd if=/dev/zero of="$tmp" bs=1048576 count=$(( (BYTES + 1048575) / 1048576 )) status=none 2>/dev/null || true
  truncate -s "$BYTES" "$tmp"
  curl -sS --proxytunnel --proxy "http://${LISTEN_ADDR}" \
    -X POST -o /dev/null -w '%{time_total}' \
    --data-binary "@${tmp}" \
    "http://bench-upstream:9000/upload"
  rm -f "$tmp"
}

bytes_time_to_mbps() {
  awk -v b="${1:-0}" -v t="${2:-0}" 'BEGIN {
    if (t <= 0) { printf "0"; exit }
    printf "%.6f", (b * 8.0) / t / 1000000.0
  }'
}

json_mbps_stats() {
  jq -n --argjson arr "$1" '
    ($arr | map tonumber) as $a |
    ($a | sort) as $s |
    ($s | length) as $n |
    if $n == 0 then
      {avg:0,median:0,p95:0,min:0,max:0}
    else
      ($s | add / $n) as $avg |
      (($s[($n/2|floor)] + $s[(($n-1)/2|floor)]) / 2) as $med |
      ($s[ (($n-1) * 0.95) | floor ]) as $p95 |
      {avg:$avg, median:$med, p95:$p95, min:$s[0], max:$s[$n-1]}
    end
  '
}

DOWN_TIMES='[]'
UP_TIMES='[]'
for ((i = 0; i < RUNS; i++)); do
  DT="$(run_download_once)"
  UT="$(run_upload_once)"
  DOWN_TIMES="$(echo "$DOWN_TIMES" | jq --arg d "$DT" '. + [$d|tonumber]')"
  UP_TIMES="$(echo "$UP_TIMES" | jq --arg u "$UT" '. + [$u|tonumber]')"
done

DOWN_MBPS_JSON='[]'
UP_MBPS_JSON='[]'
for ((i = 0; i < RUNS; i++)); do
  dt="$(echo "$DOWN_TIMES" | jq -r ".[$i]")"
  ut="$(echo "$UP_TIMES" | jq -r ".[$i]")"
  dm="$(bytes_time_to_mbps "$BYTES" "$dt")"
  um="$(bytes_time_to_mbps "$BYTES" "$ut")"
  DOWN_MBPS_JSON="$(echo "$DOWN_MBPS_JSON" | jq --argjson x "$dm" '. + [$x]')"
  UP_MBPS_JSON="$(echo "$UP_MBPS_JSON" | jq --argjson x "$um" '. + [$x]')"
done

DS="$(json_mbps_stats "$DOWN_MBPS_JSON")"
US="$(json_mbps_stats "$UP_MBPS_JSON")"

jq -n \
  --argjson down "$DS" \
  --argjson up "$US" \
  --argjson bytes "$BYTES" \
  --argjson runs "$RUNS" \
  --argjson down_times "$DOWN_TIMES" \
  --argjson up_times "$UP_TIMES" \
  '{
    bytes_per_run: $bytes,
    runs: $runs,
    download_s: $down_times,
    upload_s: $up_times,
    download_mbps: $down,
    upload_mbps: $up
  }' >"${BANDWIDTH_JSON_OUT:-/tmp/proxy-part-bandwidth.json}"
