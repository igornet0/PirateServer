#!/usr/bin/env bash
# L7 Anti-DDoS harness: burst, constant RPS, high concurrency, slowloris-like (authorized targets only).
#
#   make test-anti-ddos URL=http://192.168.0.30
#   URL=https://host CONFIRM_PUBLIC=yes ./scripts/test-anti-ddos.sh
#
# Optional env: PATH_SUFFIX INSECURE SKIP_SLOW CONFIRM_PUBLIC REPORT_JSON SKIP_INSTALL FORCE_CURL
#   BURST_N BURST_CONC CONSTANT_RPS DURATION_SEC HIGH_CONCURRENCY HIGH_DURATION_SEC
#   SLOW_CONNECTIONS PARALLEL
# Public internet targets: CONFIRM_PUBLIC=yes (or interactive YES). CI: exit 1 when unstable.
#
# Exit: 0 if stable enough; 1 if unstable / errors (timeouts) dominate or invalid URL.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/antiddos-common.sh
source "$ROOT/scripts/lib/antiddos-common.sh"

URL_RAW="${URL:-${1:-}}"
if [[ -z "${URL_RAW// }" ]]; then
  echo "usage: URL=http://host:port $0" >&2
  echo "   or: make test-anti-ddos URL=http://192.168.0.30" >&2
  exit 1
fi

BASE="${URL_RAW%/}"
PATH_SUFFIX="${PATH_SUFFIX:-/}"
INSECURE="${INSECURE:-0}"
SKIP_SLOW="${SKIP_SLOW:-0}"
CONFIRM_PUBLIC="${CONFIRM_PUBLIC:-}"
REPORT_JSON="${REPORT_JSON:-}"
BURST_N="${BURST_N:-1200}"
BURST_CONC="${BURST_CONC:-80}"
CONSTANT_RPS="${CONSTANT_RPS:-30}"
DURATION_SEC="${DURATION_SEC:-45}"
HIGH_CONCURRENCY="${HIGH_CONCURRENCY:-200}"
HIGH_DURATION_SEC="${HIGH_DURATION_SEC:-30}"
SLOW_CONNECTIONS="${SLOW_CONNECTIONS:-3}"
PARALLEL="${PARALLEL:-60}"

ANTIDDOS_HOST="$(python3 -c "from urllib.parse import urlparse; import sys; u=urlparse(sys.argv[1]); print(u.hostname or '')" "$BASE")"
if [[ -z "$ANTIDDOS_HOST" ]]; then
  echo "Invalid URL (no host): $BASE" >&2
  exit 1
fi

case "$BASE" in
  http://* | https://*) ;;
  *)
    echo "URL must start with http:// or https://" >&2
    exit 1
    ;;
esac

if ! antiddos_guard_target "$BASE" "$CONFIRM_PUBLIC"; then
  exit 1
fi

TARGET="${BASE}${PATH_SUFFIX}"
echo ""
echo "=== Anti-DDoS harness ==="
echo "target locked to: $TARGET"
echo ""

if [[ "$INSECURE" == "1" ]]; then
  export FORCE_CURL=1
  echo "INSECURE=1: using curl for TLS with -k (hey/wrk skipped for HTTPS)."
fi

antiddos_resolve_load_tool
if [[ "${DEGRADED_MODE:-0}" == "1" ]]; then
  echo "load tool: ${ANTIDDOS_LOAD_TOOL} (degraded / curl fallback)"
else
  echo "load tool: ${ANTIDDOS_LOAD_TOOL}"
fi
echo ""

WRK_LUA="$ROOT/scripts/wrk_status.lua"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

# Parallel curl: writes lines "code time_total"
run_curl_lines() {
  local n="$1"
  local p="$2"
  local out="$3"
  : >"$out"
  export TARGET INSECURE
  # xargs may return 123 if any worker exited non-zero; curl is guarded with || true, but keep the pipeline non-fatal.
  seq 1 "$n" | xargs -P "$p" -n 1 env TARGET="$TARGET" INSECURE="$INSECURE" bash -c '
    if [[ "$INSECURE" == "1" ]]; then K=( -k ); else K=(); fi
    UA="Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36"
    case $((RANDOM % 4)) in
      0) UA="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15" ;;
      1) UA="curl/8.5.0" ;;
      2) UA="Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0" ;;
    esac
    curl -sS --max-redirs 0 --connect-timeout 5 --max-time 90 "${K[@]}" -o /dev/null -w "%{http_code} %{time_total}\n" \
      -H "User-Agent: $UA" \
      -H "Accept-Language: en-US,en;q=0.9" \
      "$TARGET" >>"$1" 2>/dev/null || true
  ' _ "$out" || true
}

antiddos_parse_wrk_status_line() {
  local line="$1"
  local rest="${line#WRK_STATUS_DIST:}"
  local c429=0 c403=0 c503=0 total=0
  local tok k v
  for tok in $rest; do
    k="${tok%%:*}"
    v="${tok##*:}"
    total=$((total + v))
    case "$k" in
      429) c429=$v ;;
      403) c403=$v ;;
      503) c503=$v ;;
    esac
  done
  echo "$total $c429 $c403 $c503 $rest"
}

RL_ANY=0
BL_ANY=0
UNST_ANY=0

record_flags() {
  local total="$1"
  local c429="$2"
  local c403="$3"
  local c503="$4"
  local c000="$5"
  local err_pct="$6"
  local _flags
  _flags="$(antiddos_flags_from_metrics "$total" "$c429" "$c403" "$c503" "$c000" "$err_pct")" || _flags="0 0 0"
  read -r f_rl f_blk f_uns <<<"$_flags" || true
  [[ "$f_rl" == "1" ]] && RL_ANY=1
  [[ "$f_blk" == "1" ]] && BL_ANY=1
  [[ "$f_uns" == "1" ]] && UNST_ANY=1
}

# shellcheck disable=SC2034
fill_from_agg() {
  local agg="$1"
  read -r tot ok2 err err_pct avg_t max_t c429 c403 c503 c000 <<<"$agg" || true
  tot="${tot:-0}"
  ok2="${ok2:-0}"
  err="${err:-0}"
  err_pct="${err_pct:-0}"
  avg_t="${avg_t:-0}"
  max_t="${max_t:-0}"
  c429="${c429:-0}"
  c403="${c403:-0}"
  c503="${c503:-0}"
  c000="${c000:-0}"
  _AGG_TOT="$tot"
  _AGG_ERR_PCT="$err_pct"
  _AGG_AVG_MS="$(awk -v a="$avg_t" 'BEGIN{printf "%.1f", a*1000}')"
  _AGG_MAX_MS="$(awk -v a="$max_t" 'BEGIN{printf "%.1f", a*1000}')"
  _AGG_C429="$c429"
  _AGG_C403="$c403"
  _AGG_C503="$c503"
  _AGG_C000="$c000"
  _AGG_CODES="200:${ok2} 429:${c429} 403:${c403} 503:${c503} fail:${err} 000:${c000}"
}

# --- Burst
echo "── Burst (${BURST_N} requests, concurrency ${BURST_CONC}) ──"
BURST_TOOL="curl"
BURST_FILE="$TMPDIR/burst.json"
BURST_LINES="$TMPDIR/burst.lines"
burst_done=0

if [[ "$ANTIDDOS_LOAD_TOOL" == "hey" ]] && [[ "$INSECURE" != "1" ]]; then
  set +e
  hey -n "$BURST_N" -c "$BURST_CONC" -o json -disable-redirects "$TARGET" >"$BURST_FILE" 2>"$TMPDIR/burst.err"
  hey_rc=$?
  set -e
  if [[ "${hey_rc:-1}" -eq 0 ]] && [[ -s "$BURST_FILE" ]]; then
    burst_parse="$(antiddos_parse_hey_json "$BURST_FILE")"
    if [[ -n "$burst_parse" ]] && read -r rps avg_s slow_s tot err_pct c429 c403 c503 diststr <<<"$burst_parse"; then
      BURST_TOOL="hey"
      BURST_RPS="$rps"
      BURST_AVG_MS="$(awk -v a="${avg_s:-0}" 'BEGIN{printf "%.1f", (a+0)*1000}')"
      BURST_MAX_MS="$(awk -v a="${slow_s:-0}" 'BEGIN{printf "%.1f", (a+0)*1000}')"
      BURST_ERR_PCT="$err_pct"
      BURST_CODES="$diststr"
      tot_i="${tot%.*}"
      [[ -z "$tot_i" ]] && tot_i=0
      record_flags "$tot_i" "$c429" "$c403" "$c503" "0" "$err_pct"
      burst_done=1
    else
      echo "  hey burst: could not parse JSON (see $TMPDIR/burst.err); falling back to curl."
    fi
  else
    echo "  hey burst failed (rc=${hey_rc:-?}); falling back to curl."
  fi
fi

if [[ "$burst_done" == "0" ]] && [[ "$ANTIDDOS_LOAD_TOOL" == "wrk" ]] && [[ "$INSECURE" != "1" ]]; then
  set +e
  wrk -t4 -c"$BURST_CONC" -d8s --timeout 10s -s "$WRK_LUA" "$TARGET" >"$TMPDIR/wrk_b.txt" 2>&1
  wrk_rc=$?
  set -e
  if [[ "$wrk_rc" == 0 ]]; then
    BURST_TOOL="wrk"
    BURST_RPS="$(awk '/Requests\/sec:/{print $2; exit}' "$TMPDIR/wrk_b.txt" 2>/dev/null || true)"
    latline="$(awk '/^[[:space:]]*Latency[[:space:]]/{print; exit}' "$TMPDIR/wrk_b.txt" 2>/dev/null || true)"
    BURST_AVG_MS="$(echo "$latline" | awk '{print $2}' | tr -d 'ms' | awk '{print $1}')"
    BURST_MAX_MS="$(echo "$latline" | awk '{print $4}' | tr -d 'ms' | awk '{print $1}')"
    stline="$(awk '/WRK_STATUS_DIST:/{print; exit}' "$TMPDIR/wrk_b.txt" 2>/dev/null || true)"
    read -r tot c429 c403 c503 rest <<<"$(antiddos_parse_wrk_status_line "${stline:-}")" || true
    nons=0
    if [[ "$tot" =~ ^[0-9]+$ ]] && [[ "$tot" -gt 0 ]]; then
      c200="$(echo "$rest" | grep -oE '(^|[[:space:]])200:[0-9]+' | head -1 | cut -d: -f2)"
      c200="${c200:-0}"
      nons=$((tot - c200))
      [[ "$nons" -lt 0 ]] && nons=0
      BURST_ERR_PCT="$(awk -v e="$nons" -v n="$tot" 'BEGIN{ printf "%.2f", 100.0*e/n}')"
    else
      BURST_ERR_PCT="0"
      tot=0
    fi
    BURST_CODES="$rest"
    record_flags "$tot" "$c429" "$c403" "$c503" "0" "$BURST_ERR_PCT"
    burst_done=1
  fi
fi

if [[ "$burst_done" == "0" ]]; then
  run_curl_lines "$BURST_N" "$BURST_CONC" "$BURST_LINES"
  agg="$(antiddos_aggregate_from_lines <"$BURST_LINES")"
  fill_from_agg "$agg"
  BURST_TOT="$_AGG_TOT"
  BURST_ERR_PCT="$_AGG_ERR_PCT"
  BURST_AVG_MS="$_AGG_AVG_MS"
  BURST_MAX_MS="$_AGG_MAX_MS"
  BURST_CODES="$_AGG_CODES"
  BURST_RPS="$(awk -v n="$_AGG_TOT" -v d=8 'BEGIN{ if(d>0) printf "%.1f", n/d; else print 0}')"
  BURST_TOOL="curl"
  record_flags "$_AGG_TOT" "$_AGG_C429" "$_AGG_C403" "$_AGG_C503" "$_AGG_C000" "$_AGG_ERR_PCT"
fi

echo "  tool: $BURST_TOOL  RPS: ${BURST_RPS:-0}  err%: ${BURST_ERR_PCT:-0}  codes: ${BURST_CODES:-}"
echo ""

# --- Constant
echo "── Constant load (${CONSTANT_RPS} rps, ${DURATION_SEC}s) ──"
CONST_LINES="$TMPDIR/const.lines"
: >"$CONST_LINES"
END_TS=$(($(date +%s) + DURATION_SEC))
while [[ $(date +%s) -lt $END_TS ]]; do
  for ((j = 0; j < CONSTANT_RPS; j++)); do
    (
      if [[ "$INSECURE" == "1" ]]; then K=( -k ); else K=(); fi
      curl -sS --max-redirs 0 --connect-timeout 5 --max-time 90 "${K[@]}" -o /dev/null -w "%{http_code} %{time_total}\n" \
        -H "User-Agent: Mozilla/5.0 (Windows NT 10.0)" \
        "$TARGET" >>"$CONST_LINES" 2>/dev/null || true
    ) &
  done
  wait
  sleep 1
done
agg="$(antiddos_aggregate_from_lines <"$CONST_LINES")"
fill_from_agg "$agg"
CONST_TOT="$_AGG_TOT"
CONST_ERR_PCT="$_AGG_ERR_PCT"
CONST_AVG_MS="$_AGG_AVG_MS"
CONST_MAX_MS="$_AGG_MAX_MS"
CONST_CODES="$_AGG_CODES"
CONST_RPS="$(awk -v n="$_AGG_TOT" -v d="$DURATION_SEC" 'BEGIN{ if(d>0) printf "%.1f", n/d; else print 0}')"
CONST_TOOL="curl"
read -r _cf_rl _cf_blk CONST_UNST <<<"$(antiddos_flags_from_metrics "$_AGG_TOT" "$_AGG_C429" "$_AGG_C403" "$_AGG_C503" "$_AGG_C000" "$_AGG_ERR_PCT")" || true
record_flags "$_AGG_TOT" "$_AGG_C429" "$_AGG_C403" "$_AGG_C503" "$_AGG_C000" "$_AGG_ERR_PCT"
CONST_STABLE="YES"
[[ "$CONST_UNST" == "1" ]] && CONST_STABLE="NO"
echo "  RPS: $CONST_RPS  err%: ${CONST_ERR_PCT:-0}  stable: $CONST_STABLE"
echo ""

# --- High concurrency
echo "── High concurrency (${HIGH_CONCURRENCY} conn, ${HIGH_DURATION_SEC}s) ──"
# Enough requests to keep workers busy for the whole duration (tunable via env).
HIGH_N=$((HIGH_CONCURRENCY * HIGH_DURATION_SEC * 4))
[[ "$HIGH_N" -lt 20 ]] && HIGH_N=20
HIGH_FILE="$TMPDIR/high.json"
high_done=0
H_TOOL="curl"

if [[ "$ANTIDDOS_LOAD_TOOL" == "hey" ]] && [[ "$INSECURE" != "1" ]]; then
  set +e
  hey -n "$HIGH_N" -c "$HIGH_CONCURRENCY" -o json -disable-redirects -z "${HIGH_DURATION_SEC}s" "$TARGET" >"$HIGH_FILE" 2>/dev/null
  hey_rc=$?
  set -e
  if [[ "${hey_rc:-1}" -eq 0 ]] && [[ -s "$HIGH_FILE" ]]; then
    high_parse="$(antiddos_parse_hey_json "$HIGH_FILE")"
    if [[ -n "$high_parse" ]] && read -r rps avg_s slow_s tot err_pct c429 c403 c503 diststr <<<"$high_parse"; then
      H_TOOL="hey"
      H_RPS="$rps"
      H_AVG_MS="$(awk -v a="${avg_s:-0}" 'BEGIN{printf "%.1f", (a+0)*1000}')"
      H_MAX_MS="$(awk -v a="${slow_s:-0}" 'BEGIN{printf "%.1f", (a+0)*1000}')"
      H_ERR_PCT="$err_pct"
      H_CODES="$diststr"
      tot_i="${tot%.*}"
      [[ -z "$tot_i" ]] && tot_i=0
      record_flags "$tot_i" "$c429" "$c403" "$c503" "0" "$err_pct"
      high_done=1
    fi
  fi
fi

if [[ "$high_done" == "0" ]] && [[ "$ANTIDDOS_LOAD_TOOL" == "wrk" ]] && [[ "$INSECURE" != "1" ]]; then
  set +e
  wrk -t4 -c"$HIGH_CONCURRENCY" -d"${HIGH_DURATION_SEC}s" --timeout 15s -s "$WRK_LUA" "$TARGET" >"$TMPDIR/wrk_h.txt" 2>&1
  wrk_rc=$?
  set -e
  if [[ "$wrk_rc" == 0 ]]; then
    H_TOOL="wrk"
    H_RPS="$(awk '/Requests\/sec:/{print $2; exit}' "$TMPDIR/wrk_h.txt" 2>/dev/null || true)"
    latline="$(awk '/^[[:space:]]*Latency[[:space:]]/{print; exit}' "$TMPDIR/wrk_h.txt" 2>/dev/null || true)"
    H_AVG_MS="$(echo "$latline" | awk '{print $2}' | tr -d 'ms')"
    H_MAX_MS="$(echo "$latline" | awk '{print $4}' | tr -d 'ms')"
    stline="$(awk '/WRK_STATUS_DIST:/{print; exit}' "$TMPDIR/wrk_h.txt" 2>/dev/null || true)"
    read -r tot c429 c403 c503 rest <<<"$(antiddos_parse_wrk_status_line "${stline:-}")" || true
    nons=0
    if [[ "$tot" =~ ^[0-9]+$ ]] && [[ "$tot" -gt 0 ]]; then
      c200="$(echo "$rest" | grep -oE '(^|[[:space:]])200:[0-9]+' | head -1 | cut -d: -f2)"
      c200="${c200:-0}"
      nons=$((tot - c200))
      [[ "$nons" -lt 0 ]] && nons=0
      H_ERR_PCT="$(awk -v e="$nons" -v n="$tot" 'BEGIN{ printf "%.2f", 100.0*e/n}')"
    else
      H_ERR_PCT="0"
      tot=0
    fi
    H_CODES="$rest"
    record_flags "$tot" "$c429" "$c403" "$c503" "0" "$H_ERR_PCT"
    high_done=1
  fi
fi

if [[ "$high_done" == "0" ]]; then
  run_curl_lines "$HIGH_N" "$HIGH_CONCURRENCY" "$TMPDIR/high.lines"
  agg="$(antiddos_aggregate_from_lines <"$TMPDIR/high.lines")"
  fill_from_agg "$agg"
  H_RPS="$(awk -v n="$_AGG_TOT" -v d="$HIGH_DURATION_SEC" 'BEGIN{ if(d>0) printf "%.1f", n/d; else print 0}')"
  H_ERR_PCT="$_AGG_ERR_PCT"
  H_AVG_MS="$_AGG_AVG_MS"
  H_MAX_MS="$_AGG_MAX_MS"
  H_CODES="$_AGG_CODES"
  H_TOOL="curl"
  record_flags "$_AGG_TOT" "$_AGG_C429" "$_AGG_C403" "$_AGG_C503" "$_AGG_C000" "$_AGG_ERR_PCT"
fi

echo "  tool: $H_TOOL  RPS: ${H_RPS:-0}  err%: ${H_ERR_PCT:-0}"
echo ""

# --- Slow
SLOW_NOTE=""
if [[ "$SKIP_SLOW" != "1" ]] && command -v python3 >/dev/null 2>&1; then
  echo "── Slow (slowloris-like, ${SLOW_CONNECTIONS} connections) ──"
  python3 - "$TARGET" "$SLOW_CONNECTIONS" "$INSECURE" <<'PY'
import os, socket, ssl, sys, time, random
from urllib.parse import urlparse

def main():
    raw = sys.argv[1]
    n = int(sys.argv[2])
    insecure = sys.argv[3] == "1"
    u = urlparse(raw)
    if not u.scheme or not u.hostname:
        print("bad URL", file=sys.stderr)
        sys.exit(1)
    port = u.port or (443 if u.scheme == "https" else 80)
    host = u.hostname
    path = u.path or "/"
    if u.query:
        path += "?" + u.query
    use_tls = u.scheme == "https"
    uas = [
        "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36",
        "curl/8.5.0",
        "slow-test/1.0",
    ]
    n_ok = 0
    for _i in range(n):
        try:
            s = socket.create_connection((host, port), timeout=20)
        except OSError as e:
            print("connect failed:", e)
            continue
        if use_tls:
            ctx = ssl.create_default_context()
            if insecure:
                ctx.check_hostname = False
                ctx.verify_mode = ssl.CERT_NONE
            s = ctx.wrap_socket(s, server_hostname=host)
        ua = random.choice(uas)
        req = (
            f"GET {path} HTTP/1.1\r\nHost: {host}\r\n"
            f"User-Agent: {ua}\r\n"
            "Accept: */*\r\n"
            "Connection: keep-alive\r\n"
        )
        for part in [req, "\r\n"]:
            s.sendall(part.encode())
            time.sleep(0.4)
        time.sleep(6)
        try:
            s.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        s.close()
        n_ok += 1
    print(f"slow clients finished: {n_ok} connections")

if __name__ == "__main__":
    main()
PY
  SLOW_NOTE="(see server logs / limit_conn)"
  echo "  $SLOW_NOTE"
  echo ""
fi

TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

echo ""
echo "=== Anti-DDoS Test Report ==="
echo ""
echo "Target: $TARGET"
echo "Time: $TS"
if [[ "${DEGRADED_MODE:-0}" == "1" ]]; then
  echo "Tool chain: ${ANTIDDOS_LOAD_TOOL} + curl fallback"
else
  echo "Tool chain: ${ANTIDDOS_LOAD_TOOL}"
fi
echo ""
echo "Burst:"
echo "  RPS: ${BURST_RPS:-0}"
echo "  Latency avg/max: ${BURST_AVG_MS:-0} / ${BURST_MAX_MS:-0} ms"
echo "  Errors: ${BURST_ERR_PCT:-0}%"
echo "  HTTP codes: ${BURST_CODES:-}"
echo ""
echo "Constant load:"
echo "  RPS: ${CONST_RPS:-0}"
echo "  Latency avg/max: ${CONST_AVG_MS:-0} / ${CONST_MAX_MS:-0} ms"
echo "  Errors: ${CONST_ERR_PCT:-0}%"
echo "  Stable: ${CONST_STABLE:-YES}"
echo ""
echo "High concurrency:"
echo "  RPS: ${H_RPS:-0}"
echo "  Latency avg/max: ${H_AVG_MS:-0} / ${H_MAX_MS:-0} ms"
echo "  Errors: ${H_ERR_PCT:-0}%"
echo ""
if [[ -n "$SLOW_NOTE" ]]; then
  echo "Slow:"
  echo "  $SLOW_NOTE"
  echo ""
fi

if [[ "$RL_ANY" == "1" ]]; then
  echo "[OK] Rate limiting detected (429/503 patterns)"
else
  echo "[WARN] No rate limiting detected (no significant 429/503 in sample)"
fi
if [[ "$BL_ANY" == "1" ]]; then
  echo "[OK] Blocking / 403 observed"
else
  echo "[WARN] No blocking (403) detected"
fi
if [[ "$UNST_ANY" == "1" ]]; then
  echo "[FAIL] Server unstable under load (timeouts / high error rate)"
else
  echo "[OK] Server did not show extreme instability in this sample"
fi

echo ""
echo "Conclusion:"
if [[ "$RL_ANY" == "1" ]] && [[ "$UNST_ANY" == "0" ]]; then
  echo "  Protection appears active (rate limiting seen, no extreme instability)."
elif [[ "$UNST_ANY" == "1" ]]; then
  echo "  Needs tuning or server struggled (high errors/timeouts)."
else
  echo "  Protection may not be tuned or limits are above this test (no strong 429/503 signal)."
fi
echo ""

if [[ -n "$REPORT_JSON" ]]; then
  export TS TARGET ANTIDDOS_LOAD_TOOL DEGRADED_MODE REPORT_JSON
  export BURST_TOOL BURST_RPS BURST_AVG_MS BURST_MAX_MS BURST_ERR_PCT BURST_CODES
  export CONST_RPS CONST_AVG_MS CONST_MAX_MS CONST_ERR_PCT CONST_STABLE
  export H_TOOL H_RPS H_AVG_MS H_MAX_MS H_ERR_PCT H_CODES
  export RL_ANY BL_ANY UNST_ANY
  python3 - <<'PY'
import json, os
def fnum(x):
    try:
        return float(x)
    except Exception:
        return None

doc = {
    "harness_version": "1",
    "target": os.environ.get("TARGET", ""),
    "timestamp": os.environ.get("TS", ""),
    "load_tool": os.environ.get("ANTIDDOS_LOAD_TOOL", ""),
    "degraded": os.environ.get("DEGRADED_MODE", "0") not in ("", "0"),
    "scenarios": {
        "burst": {
            "tool": os.environ.get("BURST_TOOL", ""),
            "rps": fnum(os.environ.get("BURST_RPS", "0")),
            "latency_avg_ms": fnum(os.environ.get("BURST_AVG_MS", "0")),
            "latency_max_ms": fnum(os.environ.get("BURST_MAX_MS", "0")),
            "error_pct": fnum(os.environ.get("BURST_ERR_PCT", "0")),
            "codes": os.environ.get("BURST_CODES", ""),
        },
        "constant": {
            "rps": fnum(os.environ.get("CONST_RPS", "0")),
            "latency_avg_ms": fnum(os.environ.get("CONST_AVG_MS", "0")),
            "latency_max_ms": fnum(os.environ.get("CONST_MAX_MS", "0")),
            "error_pct": fnum(os.environ.get("CONST_ERR_PCT", "0")),
            "stable": os.environ.get("CONST_STABLE", ""),
        },
        "concurrency": {
            "tool": os.environ.get("H_TOOL", ""),
            "rps": fnum(os.environ.get("H_RPS", "0")),
            "latency_avg_ms": fnum(os.environ.get("H_AVG_MS", "0")),
            "latency_max_ms": fnum(os.environ.get("H_MAX_MS", "0")),
            "error_pct": fnum(os.environ.get("H_ERR_PCT", "0")),
            "codes": os.environ.get("H_CODES", ""),
        },
    },
    "analysis": {
        "rate_limit_detected": os.environ.get("RL_ANY", "0") == "1",
        "blocking_detected": os.environ.get("BL_ANY", "0") == "1",
        "unstable": os.environ.get("UNST_ANY", "0") == "1",
    },
}
path = os.environ["REPORT_JSON"]
with open(path, "w") as fp:
    json.dump(doc, fp, indent=2)
PY
  echo "JSON report written to $REPORT_JSON"
fi

if [[ "$UNST_ANY" == "1" ]]; then
  exit 1
fi
exit 0
