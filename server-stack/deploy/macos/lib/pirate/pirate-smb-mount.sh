#!/usr/bin/env bash
# macOS: SMB/CIFS через mount_smbfs не совпадает с Linux cifs-utils; v1 — явное сообщение.
# Для продакшена с внешними SMB-шарами используйте Linux-хост или настройте mount вручную.
set -euo pipefail
die() {
  echo "pirate-smb-mount: $*" >&2
  exit 1
}

if [[ "${EUID:-0}" -ne 0 ]]; then
  die "must run as root (use sudo)"
fi

MOUNT_POINT="${1:-}"
UNC="${2:-}"
CRED="${3:-}"

[[ -n "$MOUNT_POINT" && -n "$UNC" && -n "$CRED" ]] || die "usage: pirate-smb-mount.sh <mount_point> <//host/share> <credentials_file>"

if [[ ! "$MOUNT_POINT" =~ ^/var/lib/pirate/db-mounts/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]; then
  die "invalid mount_point"
fi

if [[ ! "$UNC" =~ ^//[a-zA-Z0-9._-]+/[a-zA-Z0-9.$_-]+$ ]]; then
  die "invalid UNC (expected //host/share)"
fi

if [[ ! "$CRED" =~ ^/var/lib/pirate/db-mounts/\.creds/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.cred$ ]]; then
  die "invalid credentials path"
fi

[[ -f "$CRED" ]] || die "credentials file missing"
[[ -r "$CRED" ]] || die "credentials file not readable"

die "macOS v1: автоматический SMB-mount из control-api не реализован (см. server-stack/deploy/macos/README.md). Используйте Linux-сервер или смонтируйте шару вручную."
