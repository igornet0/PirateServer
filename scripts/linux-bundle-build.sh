#!/usr/bin/env bash
# Build Linux tar.gz bundle (amd64 or aarch64). Used by build-linux-bundle.sh / build-arm64-linux-bundle.sh.
# Usage: ./scripts/linux-bundle-build.sh amd64|arm64
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

ARCH="${1:-amd64}"
case "${ARCH,,}" in
  amd64|x86_64)
    TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
    STAGE_DIR_NAME="pirate-linux-amd64"
    OUT_PREFIX="pirate-linux-amd64"
    ;;
  arm64|aarch64)
    TARGET_TRIPLE="${TARGET_TRIPLE:-aarch64-unknown-linux-gnu}"
    STAGE_DIR_NAME="pirate-linux-aarch64"
    OUT_PREFIX="pirate-linux-aarch64"
    ;;
  *)
    echo "usage: $0 amd64|arm64" >&2
    exit 1
    ;;
esac

UI_BUILD="${UI_BUILD:-1}"
case "${UI_BUILD,,}" in
  0|false|no|off) UI_BUILD=0 ;;
  *) UI_BUILD=1 ;;
esac

DIST_DIR="$REPO_ROOT/dist"
STAGE="$DIST_DIR/stage/$STAGE_DIR_NAME"
REL="$("$REPO_ROOT/scripts/read-version.sh")"
DATE_TAG="$(date +%Y%m%d)"
if [[ "$UI_BUILD" == "0" ]]; then
  OUT_TGZ="$DIST_DIR/${OUT_PREFIX}-no-ui-${REL}-${DATE_TAG}.tar.gz"
else
  OUT_TGZ="$DIST_DIR/${OUT_PREFIX}-${REL}-${DATE_TAG}.tar.gz"
fi

HOST_OS="$(uname -s)"
LINUX_BUNDLE_HOST_BUILD="${LINUX_BUNDLE_HOST_BUILD:-0}"
case "${LINUX_BUNDLE_HOST_BUILD,,}" in
  1|true|yes|on) LINUX_BUNDLE_HOST_BUILD=1 ;;
  *) LINUX_BUNDLE_HOST_BUILD=0 ;;
esac

if [[ "$HOST_OS" != "Darwin" || "$LINUX_BUNDLE_HOST_BUILD" == "1" ]]; then
  rustup target add "$TARGET_TRIPLE" >/dev/null 2>&1 || true
fi

if [[ "$UI_BUILD" == "1" ]]; then
  echo "==> frontend (npm run build)"
  (
    cd "$REPO_ROOT/server-stack/frontend"
    if [[ -f package-lock.json ]]; then
      npm ci
    else
      npm install
    fi
    npm run build
  )
else
  echo "==> frontend: skip (UI_BUILD=0, archive will contain .bundle-no-ui)"
fi

echo "==> Rust release ($TARGET_TRIPLE)"
if [[ "$HOST_OS" == "Darwin" && "$LINUX_BUNDLE_HOST_BUILD" != "1" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "error: on macOS, Docker is required to build this bundle (deploy-client links libxcb via xcap)." >&2
    echo "      Install Docker Desktop, or set LINUX_BUNDLE_HOST_BUILD=1 to use cargo-zigbuild/cargo on the host (link may fail on -lxcb)." >&2
    exit 1
  fi
  chmod +x "$REPO_ROOT/scripts/linux-bundle-build-rust-in-docker.sh" "$REPO_ROOT/scripts/linux-bundle-rust-docker-entry.sh" 2>/dev/null || true
  REPO_ROOT="$REPO_ROOT" CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
    "$REPO_ROOT/scripts/linux-bundle-build-rust-in-docker.sh" "$TARGET_TRIPLE"
elif command -v cargo-zigbuild >/dev/null 2>&1; then
  cargo zigbuild --release --target "$TARGET_TRIPLE" -p deploy-server -p control-api -p deploy-client
else
  echo "cargo-zigbuild not found; using cargo (install zig+cargo-zigbuild or cross for $TARGET_TRIPLE)"
  cargo build --release --target "$TARGET_TRIPLE" -p deploy-server -p control-api -p deploy-client
fi

BIN_DIR="$CARGO_TARGET_DIR/$TARGET_TRIPLE/release"
for b in deploy-server control-api client pirate; do
  if [[ ! -f "$BIN_DIR/$b" ]]; then
    echo "missing: $BIN_DIR/$b"
    exit 1
  fi
done

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/systemd" "$STAGE/nginx" "$STAGE/lib/pirate"

cp -a "$BIN_DIR/deploy-server" "$BIN_DIR/control-api" "$BIN_DIR/client" "$BIN_DIR/pirate" "$STAGE/bin/"
chmod +x "$STAGE/bin/"*
if [[ "$UI_BUILD" == "1" ]]; then
  mkdir -p "$STAGE/share/ui/dist"
  cp -a "$REPO_ROOT/server-stack/frontend/dist/." "$STAGE/share/ui/dist/"
else
  : >"$STAGE/.bundle-no-ui"
fi

cp "$REPO_ROOT/server-stack/deploy/ubuntu/deploy-server.service" "$STAGE/systemd/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/control-api.service" "$STAGE/systemd/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/nginx-pirate-site.conf" "$STAGE/nginx/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/nginx-pirate-site-domain.conf.in" "$STAGE/nginx/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/nginx-pirate-api-only.conf" "$STAGE/nginx/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/nginx-pirate-api-only-domain.conf.in" "$STAGE/nginx/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/env.example" "$STAGE/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/install.sh" "$STAGE/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/Makefile" "$STAGE/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/uninstall.sh" "$STAGE/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/purge-pirate-data.sh" "$STAGE/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/pirate-apply-stack-bundle.sh" "$STAGE/lib/pirate/"
shopt -s nullglob
for _f in "$REPO_ROOT/server-stack/deploy/ubuntu"/pirate-smb-*.sh "$REPO_ROOT/server-stack/deploy/ubuntu"/install-*.sh; do
  cp "$_f" "$STAGE/lib/pirate/"
done
shopt -u nullglob

chmod +x "$REPO_ROOT/scripts/write-server-stack-manifest.sh"
"$REPO_ROOT/scripts/write-server-stack-manifest.sh" "$STAGE" "$TARGET_TRIPLE" "$REPO_ROOT" "$UI_BUILD"

chmod +x "$STAGE/lib/pirate"/*.sh
chmod +x "$STAGE/install.sh" "$STAGE/uninstall.sh" "$STAGE/purge-pirate-data.sh"

rm -f "$OUT_TGZ"
mkdir -p "$DIST_DIR"
export COPYFILE_DISABLE=1
( cd "$DIST_DIR/stage" && tar -czf "$OUT_TGZ" "$STAGE_DIR_NAME" )

echo ""
echo "Done: $OUT_TGZ"
echo "release=$REL target=$TARGET_TRIPLE"
echo "scp to server: dist/${OUT_PREFIX}-*.tar.gz"
if [[ "$UI_BUILD" == "1" ]]; then
  echo "On server: tar xzf $(basename "$OUT_TGZ") && cd $STAGE_DIR_NAME && sudo ./install.sh --nginx --ui"
else
  echo "Archive has no dashboard static (.bundle-no-ui); --ui install is disabled."
  echo "On server: tar xzf $(basename "$OUT_TGZ") && cd $STAGE_DIR_NAME && sudo ./install.sh --nginx"
fi
echo "Uninstall: sudo ./uninstall.sh then sudo ./purge-pirate-data.sh [--remove-postgres] [--remove-linux-user]"
