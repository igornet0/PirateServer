#!/usr/bin/env bash
# Shared helpers for scripts/test-anti-ddos.sh (URL safety, tools, metrics, conclusions).
# shellcheck disable=SC2034,SC2207

antiddos_script_dir() {
  local _here
  _here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  echo "$_here"
}

# --- URL trust (Python: RFC1918, loopback, link-local, .local; resolve hostnames) ---
antiddos_host_trust_check() {
  local host="$1"
  python3 - "$host" <<'PY'
import ipaddress, socket, sys

def main():
    h = sys.argv[1].strip().lower()
    if not h:
        print("NEEDS_CONFIRM")
        return
    if h.endswith(".local") or h == "localhost":
        print("TRUSTED")
        return
    try:
        ip = ipaddress.ip_address(h)
        if ip.is_loopback or ip.is_private or ip.is_link_local or ip.is_reserved:
            print("TRUSTED")
        else:
            print("NEEDS_CONFIRM")
        return
    except ValueError:
        pass
    try:
        infos = socket.getaddrinfo(h, None, type=socket.SOCK_STREAM)
    except socket.gaierror:
        print("NEEDS_CONFIRM")
        return
    any_public = False
    any_ok = False
    for _fam, _ty, _pr, _cn, sa in infos:
        try:
            ip = ipaddress.ip_address(sa[0])
        except ValueError:
            continue
        any_ok = True
        if ip.is_loopback or ip.is_private or ip.is_link_local:
            continue
        any_public = True
        break
    if not any_ok:
        print("NEEDS_CONFIRM")
    elif any_public:
        print("NEEDS_CONFIRM")
    else:
        print("TRUSTED")

if __name__ == "__main__":
    main()
PY
}

antiddos_guard_target() {
  local raw="$1"
  local confirm="${2:-}"
  local trust
  trust="$(antiddos_host_trust_check "$ANTIDDOS_HOST")"
  if [[ "$trust" == "TRUSTED" ]]; then
    return 0
  fi
  echo ""
  echo "WARNING: Target host '$ANTIDDOS_HOST' is not in the private/loopback allowlist."
  echo "  Only load-test systems you own or have explicit permission to stress."
  if [[ "$confirm" == "yes" || "$confirm" == "1" || "$confirm" == "true" ]]; then
    echo "  CONFIRM_PUBLIC=yes — proceeding."
    return 0
  fi
  if [[ -t 0 ]]; then
    read -r -p "Type YES to continue: " _a
    if [[ "$_a" == "YES" ]]; then
      return 0
    fi
  fi
  echo "Aborting. Set CONFIRM_PUBLIC=yes or run interactively and type YES." >&2
  return 1
}

# --- Tool detection / install ---
antiddos_detect_os() {
  case "$(uname -s)" in
    Darwin) echo "darwin" ;;
    Linux) echo "linux" ;;
    *) echo "other" ;;
  esac
}

antiddos_ensure_cmd() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1
}

antiddos_try_install() {
  local tool="$1"
  local os
  os="$(antiddos_detect_os)"
  if [[ "${SKIP_INSTALL:-0}" == "1" ]]; then
    return 1
  fi
  if [[ "$os" == "darwin" ]] && command -v brew >/dev/null 2>&1; then
    brew install "$tool" 2>/dev/null || return 1
    return 0
  fi
  if [[ "$os" == "linux" ]]; then
    if command -v apt-get >/dev/null 2>&1; then
      case "$tool" in
        ab)
          sudo -n apt-get update -qq && sudo -n apt-get install -y -qq apache2-utils 2>/dev/null || return 1
          return 0
          ;;
        wrk)
          sudo -n apt-get install -y -qq wrk 2>/dev/null || return 1
          return 0
          ;;
      esac
    fi
  fi
  if [[ "$tool" == "hey" ]] && command -v go >/dev/null 2>&1; then
    local gbin
    gbin="$(go env GOPATH 2>/dev/null)/bin"
    if [[ -d "$gbin" ]]; then
      PATH="$gbin:$PATH"
      export PATH
    fi
    go install github.com/rakyll/hey@latest 2>/dev/null || return 1
    command -v hey >/dev/null 2>&1
    return $?
  fi
  return 1
}

antiddos_resolve_load_tool() {
  FORCE_CURL="${FORCE_CURL:-0}"
  DEGRADED_MODE=0
  if [[ "$FORCE_CURL" == "1" ]]; then
    ANTIDDOS_LOAD_TOOL="curl"
    DEGRADED_MODE=1
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  if antiddos_ensure_cmd hey; then
    ANTIDDOS_LOAD_TOOL="hey"
    DEGRADED_MODE=0
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  if antiddos_try_install hey; then
    ANTIDDOS_LOAD_TOOL="hey"
    DEGRADED_MODE=0
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  if antiddos_ensure_cmd wrk; then
    ANTIDDOS_LOAD_TOOL="wrk"
    DEGRADED_MODE=0
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  if antiddos_try_install wrk; then
    ANTIDDOS_LOAD_TOOL="wrk"
    DEGRADED_MODE=0
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  if antiddos_ensure_cmd ab; then
    ANTIDDOS_LOAD_TOOL="ab"
    DEGRADED_MODE=0
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  if antiddos_try_install ab; then
    ANTIDDOS_LOAD_TOOL="ab"
    DEGRADED_MODE=0
    export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
    return 0
  fi
  ANTIDDOS_LOAD_TOOL="curl"
  DEGRADED_MODE=1
  export ANTIDDOS_LOAD_TOOL DEGRADED_MODE
}

# --- Curl with randomized clients ---
antiddos_ua_pick() {
  local i="${1:-$RANDOM}"
  local uas=(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36"
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Version/17.2 Safari/605.1.15"
    "Mozilla/5.0 (X11; Linux x86_64; rv:121.0) Gecko/20100101 Firefox/121.0"
    "pirate-anti-ddos-test/1.0 curl"
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_2 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148"
  )
  echo "${uas[$((i % ${#uas[@]}))]}"
}

antiddos_extra_headers() {
  local i="${1:-$RANDOM}"
  case $((i % 4)) in
    0) echo -H "Accept-Language: en-US,en;q=0.9" ;;
    1) echo -H "Accept-Language: ru-RU,ru;q=0.9" ;;
    2) echo -H "Accept: text/html,application/xhtml+xml;q=0.9,*/*;q=0.8" ;;
    3) echo -H "Cache-Control: no-cache" ;;
  esac
}

# stdin: lines "http_code time_total" (time in seconds with dot)
antiddos_aggregate_from_lines() {
  awk '
  {
    code=$1+0
    t=$2+0
    total++
    if (code == 0 || $1 == "000" || $1 == "") {
      c000++
      err++
      next
    }
    if (int(code/100) == 2) ok2++
    else err++
    if (code == 429) c429++
    if (code == 403) c403++
    if (code == 503) c503++
    sumt += t
    if (t > maxt) maxt = t
  }
  END {
    errpct = (total > 0) ? (100.0 * err / total) : 0
    avgt = (total > 0) ? (sumt / total) : 0
    printf "%d %d %d %.2f %.6f %.6f %d %d %d %d\n", total, ok2, err, errpct, avgt, maxt, c429, c403, c503, c000
  }'
}

# --- Parse hey JSON (best-effort; versions differ). Always status 0 (set -e / pipefail safe). ---
antiddos_parse_hey_json() {
  local file="$1"
  if ! command -v jq >/dev/null 2>&1; then
    echo "0 0 0 0 0 0 0 0 {}"
    return 0
  fi
  local line err_pct
  line="$(
    jq -r '
    def num: if type == "number" then . else try tonumber catch 0 end;
    ( .summary // .Summary // {} ) as $s |
    ( $s.rps // $s["Requests/sec"] // $s.requestsPerSec // .rps // 0 | num ) as $rps |
    ( $s.average // $s["Average"] // .average // 0 | num ) as $avg |
    ( $s.slowest // $s["Slowest"] // .slowest // 0 | num ) as $slow |
    ( .status_code_distribution // .StatusCodeDistribution // {} ) as $dist |
    ( $dist | to_entries | map(.value | num) | add // 0 ) as $tot |
    ( $dist["429"] // 0 | num ) as $c429 |
    ( $dist["403"] // 0 | num ) as $c403 |
    ( $dist["503"] // 0 | num ) as $c503 |
    ( $dist | to_entries | map(select(.key != "200" and .key != "201" and .key != "204") | .value | num) | add // 0 ) as $nonsuccess |
    [ $rps, $avg, $slow, $tot, $nonsuccess, $c429, $c403, $c503, ($dist|tostring) ] | @tsv
  ' "$file" 2>/dev/null
  )" || line=""
  if [[ -z "$line" ]]; then
    echo "0 0 0 0 0 0 0 0 {}"
    return 0
  fi
  IFS=$'\t' read -r rps avg slow tot nonsucc c429 c403 c503 diststr <<<"$line" || true
  err_pct=0
  tot="${tot:-0}"
  nonsucc="${nonsucc:-0}"
  if [[ "$tot" =~ ^[0-9.]+$ ]] && awk -v t="$tot" 'BEGIN{ exit !(t+0 >= 1) }'; then
    err_pct="$(awk -v e="$nonsucc" -v n="$tot" 'BEGIN{ printf "%.2f", 100.0*e/n}')"
  fi
  echo "${rps:-0} ${avg:-0} ${slow:-0} ${tot:-0} ${err_pct:-0} ${c429:-0} ${c403:-0} ${c503:-0} ${diststr:-{}}"
  return 0
}

# --- Parse ab last lines ---
antiddos_parse_ab_output() {
  local file="$1"
  local rps avg tpr failed
  rps="$(grep -E 'Requests per second:' "$file" 2>/dev/null | awk '{print $4}')"
  tpr="$(grep -E 'Time per request:.*mean' "$file" | head -1 | awk '{print $4}')"
  failed="$(grep -E 'Failed requests:' "$file" 2>/dev/null | awk '{print $3}')"
  [[ -z "$failed" ]] && failed=0
  local total n
  n="$(grep -E '^Complete requests:' "$file" 2>/dev/null | awk '{print $3}')"
  total="${n:-0}"
  local err_pct=0
  [[ "$total" =~ ^[0-9]+$ ]] && [[ "$total" -gt 0 ]] && err_pct="$(awk -v f="$failed" -v n="$total" 'BEGIN{ printf "%.2f", 100.0*f/n}')"
  echo "${rps:-0} ${tpr:-0} ${failed:-0} ${total:-0} ${err_pct:-0}"
}

# Prints: rate_limit_detected block_detected unstable (each 0|1). Always exits 0 (safe for set -e).
antiddos_flags_from_metrics() {
  local total="$1"
  local c429="$2"
  local c403="$3"
  local c503="$4"
  local c000="$5"
  local err_pct="$6"
  awk -v t="$total" -v c429="$c429" -v c403="$c403" -v c503="$c503" -v c000="$c000" -v ep="$err_pct" '
    BEGIN {
      t = t + 0
      rl = (t > 0 && (c429 / t) >= 0.005) || (c503 > 0)
      blk = (c403 > 0)
      uns = (t > 0 && (c000 / t) > 0.15) || (ep + 0 > 40)
      print (rl ? 1 : 0) " " (blk ? 1 : 0) " " (uns ? 1 : 0)
    }' 2>/dev/null || echo "0 0 0"
}
