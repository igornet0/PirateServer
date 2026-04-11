#!/usr/bin/env bash
# Unmount SMB data source (run as root via sudo).
# Usage: pirate-smb-umount.sh <mount_point>
set -euo pipefail

die() {
  echo "pirate-smb-umount: $*" >&2
  exit 1
}

if [[ "${EUID:-0}" -ne 0 ]]; then
  die "must run as root (use sudo)"
fi

MOUNT_POINT="${1:-}"
[[ -n "$MOUNT_POINT" ]] || die "usage: pirate-smb-umount.sh <mount_point>"

if [[ ! "$MOUNT_POINT" =~ ^/var/lib/pirate/db-mounts/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]; then
  die "invalid mount_point"
fi

if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
  umount "$MOUNT_POINT" || umount -l "$MOUNT_POINT"
fi

rmdir "$MOUNT_POINT" 2>/dev/null || true

CRED="/var/lib/pirate/db-mounts/.creds/$(basename "$MOUNT_POINT").cred"
if [[ -f "$CRED" ]]; then
  shred -u -n 1 "$CRED" 2>/dev/null || rm -f "$CRED"
fi

exit 0
