#!/usr/bin/env bash
# Apply a staged Pirate server-stack bundle (root only). Invoked via sudo from deploy-server.
# Usage: pirate-apply-stack-bundle.sh <bundle_root_abs> <version_label>
# bundle_root: directory containing bin/deploy-server, bin/control-api (e.g. .../pirate-linux-amd64).

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "pirate-apply-stack-bundle.sh: must run as root" >&2
  exit 1
fi

BUNDLE_ROOT="${1:-}"
VERSION_LABEL="${2:-}"

if [[ -z "$BUNDLE_ROOT" || -z "$VERSION_LABEL" ]]; then
  echo "usage: pirate-apply-stack-bundle.sh <bundle_root_abs> <version_label>" >&2
  exit 1
fi

# Reject path traversal / unexpected roots (deploy-server only passes paths under /var/lib/pirate).
case "$BUNDLE_ROOT" in
  /var/lib/pirate/*) ;;
  *)
    echo "pirate-apply-stack-bundle.sh: bundle_root must be under /var/lib/pirate/" >&2
    exit 1
    ;;
esac

if [[ ! -d "$BUNDLE_ROOT" ]]; then
  echo "pirate-apply-stack-bundle.sh: not a directory: $BUNDLE_ROOT" >&2
  exit 1
fi

BIN_DIR="$BUNDLE_ROOT/bin"
for b in deploy-server control-api client; do
  if [[ ! -f "$BIN_DIR/$b" ]]; then
    echo "pirate-apply-stack-bundle.sh: missing $BIN_DIR/$b" >&2
    exit 1
  fi
done

HOST_ARCH="$(uname -m)"
BIN_ARCH="$(file -b "$BIN_DIR/deploy-server" 2>/dev/null || true)"
if [[ "$HOST_ARCH" == "aarch64" ]] && [[ "$BIN_ARCH" == *"x86-64"* ]]; then
  echo "pirate-apply-stack-bundle.sh: bundle is x86_64 but host is aarch64" >&2
  exit 1
fi

echo "==> install binaries -> /usr/local/bin"
install -m 0755 "$BIN_DIR/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_DIR/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_DIR/client" /usr/local/bin/client

UI_SRC="$BUNDLE_ROOT/share/ui/dist"
if [[ -f "$UI_SRC/index.html" ]]; then
  echo "==> frontend -> /var/lib/pirate/ui/dist"
  rm -rf /var/lib/pirate/ui/dist
  install -d -o pirate -g pirate -m 0755 /var/lib/pirate/ui
  cp -a "$UI_SRC" /var/lib/pirate/ui/dist
  chown -R pirate:pirate /var/lib/pirate/ui
  if command -v nginx >/dev/null 2>&1; then
    chmod o+x /var/lib/pirate 2>/dev/null || true
  fi
fi

SYSTEMD_SRC="$BUNDLE_ROOT/systemd"
if [[ -d "$SYSTEMD_SRC" ]]; then
  for u in deploy-server.service control-api.service; do
    if [[ -f "$SYSTEMD_SRC/$u" ]]; then
      install -m 0644 "$SYSTEMD_SRC/$u" "/etc/systemd/system/$u"
    fi
  done
  systemctl daemon-reload
fi

echo "$VERSION_LABEL" > /var/lib/pirate/server-stack-version
chown pirate:pirate /var/lib/pirate/server-stack-version
chmod 0644 /var/lib/pirate/server-stack-version

if [[ -f "$BUNDLE_ROOT/server-stack-manifest.json" ]]; then
  install -m 0644 "$BUNDLE_ROOT/server-stack-manifest.json" /var/lib/pirate/server-stack-manifest.json
  chown pirate:pirate /var/lib/pirate/server-stack-manifest.json
fi

echo "==> schedule service restarts (delayed so gRPC client can read OK first)"
# Restart deploy-server first (gRPC), then control-api (HTTP).
STAMP="$(date +%s%N)"
systemd-run --unit="pirate-restart-stack-${STAMP}" --on-active=2s \
  /usr/bin/systemctl restart deploy-server.service

systemd-run --unit="pirate-restart-ca-${STAMP}" --on-active=5s \
  /usr/bin/systemctl restart control-api.service

echo "ok: server-stack $VERSION_LABEL staged; services will restart shortly"
