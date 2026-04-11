#!/usr/bin/env bash
# Mount SMB share for PirateServer data sources (run as root via sudo).
# Usage: pirate-smb-mount.sh <mount_point> <//host/share> <credentials_file>
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

mkdir -p "$MOUNT_POINT"
chmod 755 "$MOUNT_POINT"

PIRATE_UID="$(id -u pirate 2>/dev/null || echo 1000)"
PIRATE_GID="$(id -g pirate 2>/dev/null || echo 1000)"

exec mount -t cifs "$UNC" "$MOUNT_POINT" -o "credentials=${CRED},uid=${PIRATE_UID},gid=${PIRATE_GID},file_mode=0644,dir_mode=0755,iocharset=utf8"
