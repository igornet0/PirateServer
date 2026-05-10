#!/usr/bin/env bash
# Bind-mount external directories into PIRATE_STORAGE_ROOT/volumes/<name> (root via sudo).
# control-api runs as user `pirate`: that user must own/write PIRATE_STORAGE_ROOT and `volumes/`,
# and the *source* tree must grant write (e.g. vfat/exfat: mount with uid= gid= matching `id pirate`).
# Usage:
#   pirate-storage-bind.sh bind <source_abs> <volume_name>
#   pirate-storage-bind.sh unbind <volume_name>
set -euo pipefail

die() {
  echo "pirate-storage-bind: $*" >&2
  exit 1
}

if [[ "${EUID:-0}" -ne 0 ]]; then
  die "must run as root (use sudo)"
fi

ENV_FILE="/etc/pirate-deploy.env"
STATE_DIR="/var/lib/pirate"
STATE="$STATE_DIR/storage-binds.json"

read_storage_root() {
  local ROOT="${PIRATE_STORAGE_ROOT:-}"
  if [[ -f "$ENV_FILE" ]] && grep -q '^PIRATE_STORAGE_ROOT=' "$ENV_FILE" 2>/dev/null; then
    local line
    line="$(grep '^PIRATE_STORAGE_ROOT=' "$ENV_FILE" | head -1)"
    local v="${line#PIRATE_STORAGE_ROOT=}"
    v="${v#\"}"
    v="${v%\"}"
    v="${v//[[:space:]]/}"
    if [[ -n "$v" ]]; then
      ROOT="$v"
    fi
  fi
  if [[ -z "$ROOT" ]]; then
    ROOT="/var/lib/pirate/file-storage"
  fi
  case "$ROOT" in
  *..* | *"'"*)
    die "refusing unsafe PIRATE_STORAGE_ROOT"
    ;;
  esac
  printf '%s' "$ROOT"
}

# Prefer /etc/pirate-deploy.env — `sudo` clears most env vars for this script.
read_bind_source_prefixes() {
  local v="${PIRATE_STORAGE_BIND_SOURCE_PREFIXES:-}"
  if [[ -f "$ENV_FILE" ]] && grep -q '^PIRATE_STORAGE_BIND_SOURCE_PREFIXES=' "$ENV_FILE" 2>/dev/null; then
    local line
    line="$(grep '^PIRATE_STORAGE_BIND_SOURCE_PREFIXES=' "$ENV_FILE" | head -1)"
    local x="${line#PIRATE_STORAGE_BIND_SOURCE_PREFIXES=}"
    x="${x#\"}"
    x="${x%\"}"
    x="${x//[[:space:]]/}"
    if [[ -n "$x" ]]; then
      v="$x"
    fi
  fi
  printf '%s' "$v"
}

canonical() {
  local p="$1"
  if command -v realpath >/dev/null 2>&1; then
    realpath -m "$p" 2>/dev/null || printf '%s' "$p"
  else
    readlink -f "$p" 2>/dev/null || printf '%s' "$p"
  fi
}

# vfat/exfat: entries appear owned by mount uid/gid — optional remount so user `pirate` can create/delete.
# We only use remount,uid=,gid= (no fmask on first try): some exfat drivers reject fmask on remount and misbehave (EIO).
# Set PIRATE_STORAGE_BIND_SKIP_FAT_REMOUNT=1 in /etc/pirate-deploy.env to disable. Set PIRATE_STORAGE_BIND_FAT_REMOUNT_FMASK=1
# to try fmask,dmask after a successful uid/gid remount (usually unnecessary).
read_skip_fat_remount() {
  local v="${PIRATE_STORAGE_BIND_SKIP_FAT_REMOUNT:-}"
  if [[ -f "$ENV_FILE" ]] && grep -q '^PIRATE_STORAGE_BIND_SKIP_FAT_REMOUNT=' "$ENV_FILE" 2>/dev/null; then
    local line x
    line="$(grep '^PIRATE_STORAGE_BIND_SKIP_FAT_REMOUNT=' "$ENV_FILE" | head -1)"
    x="${line#PIRATE_STORAGE_BIND_SKIP_FAT_REMOUNT=}"
    x="${x#\"}"
    x="${x%\"}"
    x="${x//[[:space:]]/}"
    [[ -n "$x" ]] && v="$x"
  fi
  printf '%s' "$v"
}

read_fat_remount_fmask() {
  local v="${PIRATE_STORAGE_BIND_FAT_REMOUNT_FMASK:-}"
  if [[ -f "$ENV_FILE" ]] && grep -q '^PIRATE_STORAGE_BIND_FAT_REMOUNT_FMASK=' "$ENV_FILE" 2>/dev/null; then
    local line x
    line="$(grep '^PIRATE_STORAGE_BIND_FAT_REMOUNT_FMASK=' "$ENV_FILE" | head -1)"
    x="${line#PIRATE_STORAGE_BIND_FAT_REMOUNT_FMASK=}"
    x="${x#\"}"
    x="${x%\"}"
    x="${x//[[:space:]]/}"
    [[ -n "$x" ]] && v="$x"
  fi
  printf '%s' "$v"
}

remount_fat_for_pirate_if_needed() {
  local src="$1"
  local uid="$2"
  local gid="$3"
  local skip
  skip="$(read_skip_fat_remount)"
  if [[ "$skip" == "1" || "${skip,,}" == "true" || "${skip,,}" == "yes" ]]; then
    return 0
  fi
  command -v findmnt >/dev/null 2>&1 || return 0
  local fst mp opts
  fst="$(findmnt -n -o FSTYPE -T "$src" 2>/dev/null | head -1)" || return 0
  mp="$(findmnt -n -o TARGET -T "$src" 2>/dev/null | head -1)" || return 0
  opts="$(findmnt -n -o OPTIONS -T "$src" 2>/dev/null | head -1)" || opts=""
  [[ -n "$fst" && -n "$mp" ]] || return 0
  fst="$(printf '%s' "$fst" | tr '[:upper:]' '[:lower:]')"
  case "$fst" in
  vfat | msdos | exfat) ;;
  *) return 0 ;;
  esac
  if echo "$opts" | grep -qE "(^|,)uid=${uid}(,|$)"; then
    return 0
  fi
  if ! mount -o "remount,uid=${uid},gid=${gid}" "$mp" 2>/dev/null; then
    echo "pirate-storage-bind: warning: remount uid/gid failed for ${mp} (${fst}); mkdir/delete as pirate may fail" >&2
    return 0
  fi
  echo "pirate-storage-bind: remounted ${mp} (${fst}) for uid=${uid} gid=${gid}" >&2
  local fm
  fm="$(read_fat_remount_fmask)"
  if [[ "$fm" == "1" || "${fm,,}" == "true" ]]; then
    mount -o "remount,uid=${uid},gid=${gid},fmask=0022,dmask=0022" "$mp" 2>/dev/null || true
  fi
}

source_allowed() {
  local src="$1"
  local root_canon="$2"
  local prefixes=()
  local raw
  raw="$(read_bind_source_prefixes)"
  if [[ -n "$raw" ]]; then
    IFS=':' read -r -a prefixes <<<"$raw"
  else
    prefixes=(/mnt /media /srv)
  fi
  local p
  for p in "${prefixes[@]}"; do
    [[ -z "$p" ]] && continue
    if [[ "$src" == "$p" || "$src" == "$p/"* ]]; then
      if [[ "$src" == "$root_canon" || "$src" == "$root_canon/"* ]]; then
        return 1
      fi
      return 0
    fi
  done
  return 1
}

volume_name_ok() {
  [[ "$1" =~ ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,62}$ ]]
}

state_read() {
  install -d -m 0755 -o root -g root "$STATE_DIR" 2>/dev/null || true
  if [[ ! -f "$STATE" ]]; then
    echo '{"version":1,"binds":[]}'
    return
  fi
  python3 - "$STATE" <<'PY' || echo '{"version":1,"binds":[]}'
import json, sys
path = sys.argv[1]
try:
    with open(path, "r", encoding="utf-8") as f:
        j = json.load(f)
    if not isinstance(j, dict) or "binds" not in j:
        raise ValueError("bad shape")
    j.setdefault("version", 1)
    j["binds"] = [b for b in j.get("binds", []) if isinstance(b, dict) and "volume" in b and "source" in b]
    print(json.dumps(j))
except Exception:
    print(json.dumps({"version": 1, "binds": []}))
PY
}

state_add() {
  local vol="$1" src="$2"
  python3 - "$STATE" "$vol" "$src" <<'PY'
import json, sys, os, tempfile
path, vol, src = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(os.path.dirname(path), mode=0o755, exist_ok=True)
binds = []
if os.path.isfile(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            j = json.load(f)
        binds = [b for b in j.get("binds", []) if isinstance(b, dict)]
    except Exception:
        binds = []
for b in binds:
    if b.get("volume") == vol:
        print("exists", file=sys.stderr)
        sys.exit(1)
binds.append({"volume": vol, "source": src})
out = {"version": 1, "binds": binds}
d = os.path.dirname(path)
fd, tmp = tempfile.mkstemp(prefix=".storage-binds.", dir=d, text=True)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=0)
        f.write("\n")
    os.chmod(tmp, 0o644)
    try:
        import grp
        g = grp.getgrnam("pirate")
        os.chown(tmp, 0, g.gr_gid)
    except Exception:
        pass
    os.replace(tmp, path)
except Exception:
    try:
        os.unlink(tmp)
    except Exception:
        pass
    raise
PY
}

state_remove() {
  local vol="$1"
  python3 - "$STATE" "$vol" <<'PY'
import json, sys, os, tempfile
path, vol = sys.argv[1], sys.argv[2]
if not os.path.isfile(path):
    sys.exit(0)
with open(path, "r", encoding="utf-8") as f:
    j = json.load(f)
binds = [b for b in j.get("binds", []) if isinstance(b, dict) and b.get("volume") != vol]
out = {"version": 1, "binds": binds}
d = os.path.dirname(path)
fd, tmp = tempfile.mkstemp(prefix=".storage-binds.", dir=d, text=True)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=0)
        f.write("\n")
    os.chmod(tmp, 0o644)
    try:
        import grp
        g = grp.getgrnam("pirate")
        os.chown(tmp, 0, g.gr_gid)
    except Exception:
        pass
    os.replace(tmp, path)
except Exception:
    try:
        os.unlink(tmp)
    except Exception:
        pass
    raise
PY
}

cmd_bind() {
  local SRC_RAW="${1:-}"
  local VOL="${2:-}"
  [[ -n "$SRC_RAW" && -n "$VOL" ]] || die "usage: bind <source_abs> <volume_name>"
  volume_name_ok "$VOL" || die "invalid volume_name"
  if echo "$(state_read)" | python3 -c "import json,sys; vol=sys.argv[1]; j=json.load(sys.stdin); sys.exit(0 if any(b.get(\"volume\")==vol for b in j.get(\"binds\",[])) else 1)" "$VOL" 2>/dev/null; then
    die "volume name already registered"
  fi
  local ROOT
  ROOT="$(read_storage_root)"
  local ROOT_CANON
  ROOT_CANON="$(canonical "$ROOT")"
  [[ -n "$ROOT_CANON" && -d "$ROOT_CANON" ]] || die "PIRATE_STORAGE_ROOT is not a directory"
  local SRC
  SRC="$(canonical "$SRC_RAW")"
  [[ -n "$SRC" && -d "$SRC" ]] || die "source is not a directory"
  source_allowed "$SRC" "$ROOT_CANON" || die "source path not allowed or under storage root"
  local PIRATE_UID PIRATE_GID
  PIRATE_UID="$(id -u pirate 2>/dev/null || echo 1000)"
  PIRATE_GID="$(id -g pirate 2>/dev/null || echo 1000)"
  remount_fat_for_pirate_if_needed "$SRC" "$PIRATE_UID" "$PIRATE_GID"
  local TARGET="$ROOT_CANON/volumes/$VOL"
  case "$TARGET" in
  "$ROOT_CANON" | "$ROOT_CANON/"*)
    ;;
  *)
    die "internal: target outside root"
    ;;
  esac
  # So user `pirate` can create/remove sibling volume dirs and manage storage via control-api.
  install -d -m 0775 -o "$PIRATE_UID" -g "$PIRATE_GID" "$ROOT_CANON/volumes"
  if mountpoint -q "$TARGET" 2>/dev/null; then
    die "target already a mountpoint: $TARGET"
  fi
  if [[ -e "$TARGET" ]]; then
    die "target path already exists (remove or pick another name): $TARGET"
  fi
  install -d -m 0775 -o "$PIRATE_UID" -g "$PIRATE_GID" "$TARGET"
  mount --bind "$SRC" "$TARGET"
  if ! state_add "$VOL" "$SRC"; then
    umount "$TARGET" || true
    rmdir "$TARGET" 2>/dev/null || true
    die "state file update failed or volume already registered"
  fi
  echo "ok: bound $SRC -> $TARGET"
}

cmd_unbind() {
  local VOL="${1:-}"
  [[ -n "$VOL" ]] || die "usage: unbind <volume_name>"
  volume_name_ok "$VOL" || die "invalid volume_name"
  local ROOT
  ROOT="$(read_storage_root)"
  local ROOT_CANON
  ROOT_CANON="$(canonical "$ROOT")"
  local TARGET="$ROOT_CANON/volumes/$VOL"
  local j
  j="$(state_read)"
  if ! echo "$j" | python3 -c "import json,sys; j=json.load(sys.stdin); sys.exit(0 if any(b.get('volume')==sys.argv[1] for b in j.get('binds',[])) else 1)" "$VOL" 2>/dev/null; then
    die "volume not in bind registry"
  fi
  if ! mountpoint -q "$TARGET" 2>/dev/null; then
    state_remove "$VOL"
    rmdir "$TARGET" 2>/dev/null || true
    die "not a mountpoint (cleaned registry): $TARGET"
  fi
  umount "$TARGET" || die "umount failed (busy?)"
  state_remove "$VOL"
  rmdir "$TARGET" 2>/dev/null || true
  echo "ok: unbound $TARGET"
}

SUB="${1:-}"
shift || true
case "$SUB" in
bind)
  cmd_bind "${1:-}" "${2:-}"
  ;;
unbind)
  cmd_unbind "${1:-}"
  ;;
*)
  die "usage: pirate-storage-bind.sh bind <source_abs> <volume_name> | pirate-storage-bind.sh unbind <volume_name>"
  ;;
esac

exit 0
