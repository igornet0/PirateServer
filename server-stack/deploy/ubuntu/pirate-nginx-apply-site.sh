#!/usr/bin/env bash
# Записывает vhost Pirate из stdin, проверяет и перезагружает nginx. Запуск: sudo … [path]
set -euo pipefail
TARGET="${1:-/etc/nginx/sites-available/pirate}"
MAX=$((256 * 1024))
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
cat >"$TMP"
SZ="$(wc -c <"$TMP" | tr -d ' ')"
if [ "$SZ" -gt "$MAX" ]; then
  echo "pirate-nginx-apply-site: content exceeds ${MAX} bytes" >&2
  exit 1
fi
install -m 0644 "$TMP" "$TARGET"
nginx -t
systemctl reload nginx
echo "ok: nginx site $TARGET applied and reloaded"
