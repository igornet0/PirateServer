#!/usr/bin/env bash
# Сборка серверного tar.gz для macOS (amd64/arm64). Читает server-stack/deploy/macos/build-config.json,
# копирует общие файлы из server-stack/deploy/ubuntu, затем артефакты из deploy/macos.
#
# Usage: ARCH=amd64|arm64 UI_BUILD=0|1 ./scripts/macos-bundle-build.sh
# Требуется сборка на macOS с установленным rust target (x86_64-apple-darwin / aarch64-apple-darwin).
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: macOS bundle must be built on Darwin (apple-darwin)." >&2
  exit 1
fi

ARCH_RAW="${ARCH:-amd64}"
case "${ARCH_RAW,,}" in
  amd64|x86_64)
    TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-apple-darwin}"
    STAGE_DIR_NAME="pirate-macos-amd64"
    OUT_PREFIX="pirate-macos-amd64"
    ;;
  arm64|aarch64)
    TARGET_TRIPLE="${TARGET_TRIPLE:-aarch64-apple-darwin}"
    STAGE_DIR_NAME="pirate-macos-arm64"
    OUT_PREFIX="pirate-macos-arm64"
    ;;
  *)
    echo "usage: ARCH=amd64|arm64 $0" >&2
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

rustup target add "$TARGET_TRIPLE" >/dev/null 2>&1 || true

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
cargo build --release --target "$TARGET_TRIPLE" -p deploy-server -p control-api -p deploy-client

BIN_DIR="$CARGO_TARGET_DIR/$TARGET_TRIPLE/release"
for b in deploy-server control-api client pirate; do
  if [[ ! -f "$BIN_DIR/$b" ]]; then
    echo "missing: $BIN_DIR/$b"
    exit 1
  fi
done

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/nginx" "$STAGE/lib/pirate" "$STAGE/launchd"

cp -a "$BIN_DIR/deploy-server" "$BIN_DIR/control-api" "$BIN_DIR/client" "$BIN_DIR/pirate" "$STAGE/bin/"
chmod +x "$STAGE/bin/"*
if [[ "$UI_BUILD" == "1" ]]; then
  mkdir -p "$STAGE/share/ui/dist"
  cp -a "$REPO_ROOT/server-stack/frontend/dist/." "$STAGE/share/ui/dist/"
else
  : >"$STAGE/.bundle-no-ui"
fi

CONFIG_JSON="$REPO_ROOT/server-stack/deploy/macos/build-config.json"
UBUNTU_DIR="$REPO_ROOT/server-stack/deploy/ubuntu"
MACOS_DEPLOY="$REPO_ROOT/server-stack/deploy/macos"

python3 <<PY
import json, os, shutil
repo = r"$REPO_ROOT"
stage = r"$STAGE"
ubuntu = os.path.join(repo, "server-stack", "deploy", "ubuntu")
with open(r"$CONFIG_JSON", "r", encoding="utf-8") as f:
    cfg = json.load(f)
for item in cfg.get("copyFromUbuntu", []):
    src = os.path.join(ubuntu, item["src"])
    dst_rel = item.get("dst", ".").strip() or "."
    dst_dir = os.path.join(stage, dst_rel)
    os.makedirs(dst_dir, exist_ok=True)
    dst = os.path.join(dst_dir, os.path.basename(src))
    shutil.copy2(src, dst)
PY

rsync -a --exclude "build-config.json" "$MACOS_DEPLOY/" "$STAGE/"

chmod +x "$STAGE/lib/pirate"/*.sh 2>/dev/null || true
chmod +x "$STAGE/install.sh" "$STAGE/uninstall.sh" "$STAGE/purge-pirate-data.sh"

chmod +x "$REPO_ROOT/scripts/write-server-stack-manifest.sh"
"$REPO_ROOT/scripts/write-server-stack-manifest.sh" "$STAGE" "$TARGET_TRIPLE" "$REPO_ROOT" "$UI_BUILD"

rm -f "$OUT_TGZ"
mkdir -p "$DIST_DIR"
export COPYFILE_DISABLE=1
( cd "$DIST_DIR/stage" && tar -czf "$OUT_TGZ" "$STAGE_DIR_NAME" )

echo ""
echo "Done: $OUT_TGZ"
echo "release=$REL target=$TARGET_TRIPLE"
echo "On Mac: tar xzf $(basename "$OUT_TGZ") && cd $STAGE_DIR_NAME && sudo ./install.sh --nginx --ui"

if [[ "${MAKE_DMG:-}" == "1" ]]; then
  OUT_DMG="$DIST_DIR/${OUT_PREFIX}-${REL}-${DATE_TAG}.dmg"
  rm -f "$OUT_DMG"
  echo "==> hdiutil -> $OUT_DMG"
  hdiutil create -volname "PirateServer-${STAGE_DIR_NAME}" -srcfolder "$STAGE" -ov -format UDZO "$OUT_DMG"
  echo "DMG: $OUT_DMG"
fi
