#!/usr/bin/env bash
set -euo pipefail

if ! command -v ufw >/dev/null 2>&1; then
  echo "ufw is not installed" >&2
  exit 1
fi

MODE="${1:-minimal}"
shift || true

ufw --force enable >/dev/null 2>&1 || true
ufw default deny incoming
ufw default allow outgoing
ufw allow 22/tcp

if [[ "$MODE" == "wan" ]]; then
  ufw allow 80/tcp
  ufw allow 443/tcp
fi

for p in "$@"; do
  [[ -z "$p" ]] && continue
  ufw allow "${p}/tcp"
done

ufw reload >/dev/null 2>&1 || true
echo "OK: firewall policy applied (${MODE})"
