#!/usr/bin/env bash
# Apply optional tc netem/tbf on primary container interface (requires root + CAP_NET_ADMIN).
# Env: NETEM_DELAY_MS, NETEM_LOSS_PERCENT, NETEM_BANDWIDTH (e.g. 50mbit).
set -euo pipefail

NETEM_DELAY_MS="${NETEM_DELAY_MS:-0}"
NETEM_LOSS_PERCENT="${NETEM_LOSS_PERCENT:-0}"
NETEM_BANDWIDTH="${NETEM_BANDWIDTH:-}"

detect_iface() {
  local d
  d="$(ip route show default 2>/dev/null | awk '{print $5; exit}')"
  if [[ -n "$d" ]]; then
    echo "$d"
    return
  fi
  for c in eth0 ens160 enp0s3; do
    if ip link show "$c" &>/dev/null; then
      echo "$c"
      return
    fi
  done
  ip -o link show | awk -F': ' '$2!="lo"{print $2; exit}'
}

cleanup_tc() {
  local dev="$1"
  tc qdisc del dev "$dev" root 2>/dev/null || true
}

apply_tc() {
  local dev="$1"
  cleanup_tc "$dev"

  local d="${NETEM_DELAY_MS:-0}"
  local l="${NETEM_LOSS_PERCENT:-0}"
  local bw="${NETEM_BANDWIDTH:-}"

  if [[ -z "$bw" && "$d" == "0" && "$l" == "0" ]]; then
    echo "network_setup: no netem/tbf (NETEM_* unset or zero)"
    return 0
  fi

  # Bandwidth only: simple TBF on root.
  if [[ -n "$bw" && "$d" == "0" && "$l" == "0" ]]; then
    tc qdisc add dev "$dev" root tbf rate "$bw" burst 32kbit latency 400ms
    echo "network_setup: dev=$dev mode=tbf rate=$bw"
    return 0
  fi

  # Delay/loss only (no bandwidth cap): netem on root.
  if [[ -z "$bw" ]]; then
    local args=()
    [[ "$d" != "0" ]] && args+=(delay "${d}ms")
    [[ "$l" != "0" ]] && args+=(loss "${l}%")
    tc qdisc add dev "$dev" root netem "${args[@]}"
    echo "network_setup: dev=$dev mode=netem delay_ms=$d loss=${l}%"
    return 0
  fi

  # Bandwidth + delay and/or loss: HTB + netem under leaf class.
  tc qdisc add dev "$dev" root handle 1: htb default 10
  tc class add dev "$dev" parent 1: classid 1:1 htb rate "$bw" ceil "$bw"
  tc class add dev "$dev" parent 1:1 classid 1:10 htb rate "$bw" ceil "$bw" prio 0
  local args=()
  [[ "$d" != "0" ]] && args+=(delay "${d}ms")
  [[ "$l" != "0" ]] && args+=(loss "${l}%")
  if [[ ${#args[@]} -gt 0 ]]; then
    tc qdisc add dev "$dev" parent 1:10 handle 20: netem "${args[@]}"
  else
    tc qdisc add dev "$dev" parent 1:10 handle 20: pfifo limit 1000
  fi
  echo "network_setup: dev=$dev mode=htb+netem rate=$bw delay_ms=$d loss=${l}%"
}

main() {
  if [[ "$(id -u)" != "0" ]]; then
    echo "network_setup: not root; skipping tc (set user: root on proxy-test service)" >&2
    return 0
  fi
  local dev
  dev="$(detect_iface)"
  if [[ -z "$dev" ]]; then
    echo "network_setup: could not detect iface; skipping" >&2
    return 0
  fi
  apply_tc "$dev"
}

main "$@"
