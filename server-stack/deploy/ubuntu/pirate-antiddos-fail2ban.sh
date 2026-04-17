#!/usr/bin/env bash
# Install/configure fail2ban jail for pirate-antiddos nginx log. Root only.
set -euo pipefail

die() {
  echo "pirate-antiddos-fail2ban: $*" >&2
  exit 1
}

[[ "${EUID:-0}" -eq 0 ]] || die "must run as root"

ENABLED="${1:-0}"
F2B_JSON="${2:-}"

FILTER_DIR="/etc/fail2ban/filter.d"
JAIL_DIR="/etc/fail2ban/jail.d"
LOG_PATH="/var/log/nginx/pirate-antiddos-error.log"

if [[ "$ENABLED" != "1" ]]; then
  rm -f "$JAIL_DIR/pirate-nginx-dos.conf" 2>/dev/null || true
  if command -v fail2ban-client >/dev/null 2>&1; then
    fail2ban-client reload 2>/dev/null || true
  fi
  echo "ok: pirate fail2ban jail removed (disabled)"
  exit 0
fi

[[ -f "$F2B_JSON" ]] || die "missing json path"

read -r BANTIME FINDTIME MAXRETRY DISABLED <<<"$(python3 -c "
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as f:
    j = json.load(f)
fb = j.get('fail2ban') or {}
if not fb.get('enabled', True):
    print('0 0 0 1')
else:
    print(int(fb.get('bantime_sec', 600)), int(fb.get('findtime_sec', 120)), int(fb.get('maxretry', 10)), 0)
" "$F2B_JSON")"

if [[ "${DISABLED:-0}" == "1" ]]; then
  rm -f "$JAIL_DIR/pirate-nginx-dos.conf" 2>/dev/null || true
  echo "ok: fail2ban disabled in config"
  exit 0
fi

export DEBIAN_FRONTEND=noninteractive
if ! command -v fail2ban-client >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq fail2ban
fi

install -d -m 0755 "$FILTER_DIR" "$JAIL_DIR"

cat >"$FILTER_DIR/pirate-nginx-dos.conf" <<'EOF'
[Definition]
failregex = ^.*\slimiting requests,.*$
            ^.*\sconnection limiting,.*$
ignoreregex =
EOF

BANACTION="iptables-multiport"
if [[ -f /etc/fail2ban/action.d/nftables-multiport.conf ]]; then
  BANACTION="nftables-multiport"
elif [[ -f /etc/fail2ban/action.d/nftables.conf ]]; then
  BANACTION="nftables"
fi

cat >"$JAIL_DIR/pirate-nginx-dos.conf" <<EOF
[pirate-nginx-dos]
enabled = true
filter = pirate-nginx-dos
logpath = $LOG_PATH
maxretry = $MAXRETRY
findtime = $FINDTIME
bantime = $BANTIME
banaction = $BANACTION
port = http,https
EOF

touch "$LOG_PATH"
chmod 0644 "$LOG_PATH" 2>/dev/null || true

systemctl enable fail2ban 2>/dev/null || true
systemctl restart fail2ban 2>/dev/null || true
fail2ban-client reload 2>/dev/null || true

echo "ok: fail2ban jail pirate-nginx-dos configured"
