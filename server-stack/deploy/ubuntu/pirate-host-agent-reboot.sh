#!/usr/bin/env bash
# Invoked via sudo from pirate-host-agent. Schedules system reboot (root only).
# Usage: pirate-host-agent-reboot.sh <delay_sec>
set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "pirate-host-agent-reboot.sh: must run as root" >&2
  exit 1
fi

DELAY_SEC="${1:-0}"
if ! [[ "$DELAY_SEC" =~ ^[0-9]+$ ]]; then
  echo "pirate-host-agent-reboot.sh: delay must be a non-negative integer" >&2
  exit 1
fi

if [[ "$DELAY_SEC" -gt 3600 ]]; then
  echo "pirate-host-agent-reboot.sh: delay_sec must be <= 3600" >&2
  exit 1
fi

if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  if [[ "$DELAY_SEC" -eq 0 ]]; then
    exec /usr/bin/systemctl reboot
  fi
  /usr/bin/systemd-run --unit=pirate-host-agent-delayed-reboot --no-block \
    /bin/sh -c "sleep ${DELAY_SEC} && /usr/bin/systemctl reboot"
else
  # macOS / BSD: no systemd
  if [[ "$DELAY_SEC" -eq 0 ]]; then
    exec /sbin/shutdown -r now
  fi
  /bin/sh -c "sleep ${DELAY_SEC} && /sbin/shutdown -r now" >/dev/null 2>&1 &
fi
