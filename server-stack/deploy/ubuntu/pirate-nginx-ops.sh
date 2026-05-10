#!/usr/bin/env bash
# Privileged nginx helpers for control-api (run via: sudo -n …).
# Subcommands: validate | validate-config PATH | reload | enable-site AVAIL ENABLED | disable-site ENABLED | apply-config TARGET [full_main]
set -euo pipefail

MAX=$((256 * 1024))
usage() {
  echo "usage: pirate-nginx-ops.sh validate|validate-config PATH|reload|enable-site AVAIL ENABLED|disable-site ENABLED|apply-config TARGET [full_main]" >&2
}

# Only allow symlink/remove under sites-enabled (matches control-api enable_site / disable_site).
assert_sites_enabled_path() {
  local p="$1"
  case "$p" in
    /etc/nginx/sites-enabled/*) return 0 ;;
    *)
      echo "pirate-nginx-ops: path must be under /etc/nginx/sites-enabled: $p" >&2
      return 1
      ;;
  esac
}

CMD="${1:-}"
shift || true

case "$CMD" in
  validate)
    exec nginx -t 2>&1
    ;;
  validate-config)
    CONF="${1:?config path required}"
    exec nginx -t -c "$CONF" 2>&1
    ;;
  reload)
    exec systemctl reload nginx
    ;;
  enable-site)
    AVAIL="${1:?sites-available path required}"
    EN="${2:?sites-enabled path required}"
    assert_sites_enabled_path "$EN" || exit 1
    ln -sf "$AVAIL" "$EN"
    ;;
  disable-site)
    EN="${1:?sites-enabled path required}"
    assert_sites_enabled_path "$EN" || exit 1
    rm -f "$EN"
    ;;
  apply-config)
    TARGET="${1:?target path required}"
    MODE="${2:-}"
    TMP="$(mktemp)"
    trap 'rm -f "$TMP"' EXIT
    cat >"$TMP"
    SZ="$(wc -c <"$TMP" | tr -d ' ')"
    if [ "$SZ" -gt "$MAX" ]; then
      echo "pirate-nginx-ops: content exceeds ${MAX} bytes" >&2
      exit 1
    fi
    nginx_test() {
      if [ "$MODE" = "full_main" ]; then
        nginx -t -c "$TARGET" 2>&1
      else
        nginx -t 2>&1
      fi
    }
    if [ ! -f "$TARGET" ]; then
      install -m 0644 "$TMP" "$TARGET"
      OUT="$(nginx_test)" || {
        rm -f "$TARGET"
        echo "$OUT" >&2
        exit 1
      }
      systemctl reload nginx
      printf '%s\n' "$OUT"
      echo "ok: created $TARGET and reloaded nginx"
      exit 0
    fi
    BACKUP="${TARGET}.pirate.bak"
    cp -a "$TARGET" "$BACKUP"
    if ! install -m 0644 "$TMP" "$TARGET"; then
      cp -a "$BACKUP" "$TARGET"
      rm -f "$BACKUP"
      exit 1
    fi
    OUT="$(nginx_test)" || {
      cp -a "$BACKUP" "$TARGET"
      rm -f "$BACKUP"
      echo "pirate-nginx-ops: nginx -t failed; reverted $TARGET" >&2
      echo "$OUT" >&2
      nginx -t 2>&1 || true
      exit 1
    }
    rm -f "$BACKUP"
    systemctl reload nginx
    printf '%s\n' "$OUT"
    echo "ok: applied $TARGET and reloaded nginx"
    ;;
  *)
    usage
    exit 2
    ;;
esac
