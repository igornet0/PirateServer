#!/usr/bin/env bash
# Create PIRATE_STORAGE_ROOT, .pirate-tmp, optional env line; cleanup stale temp uploads.
# control-api runs as user `pirate` (see control-api.service) — data dirs are root:pirate.
# Invoked as root from install.sh and pirate-apply-stack-bundle.sh after bundle sync.
set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "pirate-ensure-file-storage: must run as root" >&2
  exit 1
fi

ENV_FILE="/etc/pirate-deploy.env"
DEFAULT_ROOT="/var/lib/pirate/file-storage"
ROOT="${PIRATE_STORAGE_ROOT:-}"

# Prefer value already in /etc/pirate-deploy.env (do not `source` the whole file).
if [[ -f "$ENV_FILE" ]]; then
  if grep -q '^PIRATE_STORAGE_ROOT=' "$ENV_FILE" 2>/dev/null; then
    line="$(grep '^PIRATE_STORAGE_ROOT=' "$ENV_FILE" | head -1)"
    v="${line#PIRATE_STORAGE_ROOT=}"
    v="${v#\"}"
    v="${v%\"}"
    v="${v//[[:space:]]/}"
    if [[ -n "$v" ]]; then
      ROOT="$v"
    fi
  fi
fi
if [[ -z "$ROOT" ]]; then
  ROOT="${DEFAULT_ROOT}"
fi
case "$ROOT" in
*..* | *"'"*)
  echo "pirate-ensure-file-storage: refusing unsafe PIRATE_STORAGE_ROOT" >&2
  exit 1
  ;;
esac

echo "pirate-ensure-file-storage: PIRATE_STORAGE_ROOT=$ROOT"
install -d -m 0755 -o pirate -g pirate "$ROOT"
install -d -m 0700 -o pirate -g pirate "$ROOT/.pirate-tmp"
# Bind-mount volume names live here; group-writable so control-api (user pirate) can manage layout.
install -d -m 0775 -o pirate -g pirate "$ROOT/volumes"

# Best-effort: temp files from interrupted uploads (older than 1 day)
find "$ROOT/.pirate-tmp" -type f -mtime +1 -delete 2>/dev/null || true

if [[ -f "$ENV_FILE" ]]; then
  if ! grep -q '^PIRATE_STORAGE_ROOT=' "$ENV_FILE" 2>/dev/null; then
    {
      echo ""
      echo "# Pirate file storage (desktop; dirs created by pirate-ensure-file-storage.sh)"
      echo "PIRATE_STORAGE_ROOT=$ROOT"
    } >>"$ENV_FILE"
    chmod 0640 "$ENV_FILE" 2>/dev/null || true
    chown root:pirate "$ENV_FILE" 2>/dev/null || true
  fi
else
  echo "pirate-ensure-file-storage: note: $ENV_FILE not found — created dirs only; set PIRATE_STORAGE_ROOT= on control-api" >&2
fi

exit 0
