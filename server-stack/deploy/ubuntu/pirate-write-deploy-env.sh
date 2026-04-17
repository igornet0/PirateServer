#!/usr/bin/env bash
# Writes /etc/pirate-deploy.env from stdin (root via sudo). Used by control-api (user pirate).
# Usage: sudo /usr/local/lib/pirate/pirate-write-deploy-env.sh [/etc/pirate-deploy.env]
set -euo pipefail
TARGET="${1:-/etc/pirate-deploy.env}"
MAX=$((512 * 1024))
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
cat >"$TMP"
SZ="$(wc -c <"$TMP" | tr -d ' ')"
if [ "$SZ" -gt "$MAX" ]; then
  echo "pirate-write-deploy-env: content exceeds ${MAX} bytes" >&2
  exit 1
fi
install -m 0640 -o root -g pirate "$TMP" "$TARGET"

STAMP="$(date +%s%N)"
if command -v systemd-run >/dev/null 2>&1 && command -v systemctl >/dev/null 2>&1; then
  # Delayed so the HTTP client receives 200 before control-api restarts.
  systemd-run --unit="pirate-restart-ds-${STAMP}" --on-active=2s \
    /usr/bin/systemctl restart deploy-server.service
  systemd-run --unit="pirate-restart-ca-${STAMP}" --on-active=5s \
    /usr/bin/systemctl restart control-api.service
  echo "ok: ${TARGET} written; deploy-server and control-api will restart shortly"
else
  echo "ok: ${TARGET} written (no systemd-run; restart deploy-server and control-api manually)" >&2
fi
