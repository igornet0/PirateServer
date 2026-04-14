#!/usr/bin/env bash
# Windows server bundle (zip). Reads server-stack/deploy/windows/build-config.json,
# copies shared files from deploy/ubuntu, then deploy/windows (except build-config.json).
#
# Usage: ARCH=amd64|arm64 UI_BUILD=0|1 ./scripts/windows-bundle-build.sh
# On macOS/Linux, MSVC cross-build uses cargo-xwin (Windows SDK/CRT for clang). Set
# PIRATE_SKIP_CARGO_XWIN_INSTALL=1 to skip automatic `cargo install cargo-xwin`.
# Default XWIN_CROSS_COMPILER=clang (windows-msvc-sysroot): Apple clang does not accept
# MSVC /imsvc flags used by the clang-cl backend; override with XWIN_CROSS_COMPILER=clang-cl
# if you use LLVM with a real clang-cl (e.g. brew install llvm, PATH).
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

# --- Host + Windows MSVC cross-compile (Darwin/Linux → *-pc-windows-msvc) ---

host_windows_native() {
  case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) return 0 ;;
    *) return 1 ;;
  esac
}

host_unix_cross_windows() {
  case "$(uname -s)" in
    Darwin | Linux) return 0 ;;
    *) return 1 ;;
  esac
}

ensure_clang_for_xwin() {
  if command -v clang >/dev/null 2>&1; then
    return 0
  fi
  echo "error: clang not found in PATH (required for cargo-xwin / ring)." >&2
  echo "  macOS: xcode-select --install   or: brew install llvm" >&2
  echo "  Debian/Ubuntu: sudo apt install clang" >&2
  exit 1
}

ensure_cargo_xwin() {
  if cargo xwin --help >/dev/null 2>&1; then
    return 0
  fi
  if [[ -n "${PIRATE_SKIP_CARGO_XWIN_INSTALL:-}" ]]; then
    echo "error: cargo-xwin is not available and PIRATE_SKIP_CARGO_XWIN_INSTALL is set." >&2
    echo "  Install: cargo install --locked cargo-xwin" >&2
    exit 1
  fi
  echo "==> installing cargo-xwin (cross MSVC toolchain; accepts Microsoft SDK license)"
  cargo install --locked cargo-xwin
}

cargo_windows_release() {
  local -a cmd
  if host_windows_native; then
    cmd=(cargo build)
  elif host_unix_cross_windows; then
    echo "==> Windows cross-compile from $(uname -s): preparing clang + cargo-xwin"
    ensure_clang_for_xwin
    rustup component add llvm-tools >/dev/null 2>&1 || true
    ensure_cargo_xwin
    export XWIN_CROSS_COMPILER="${XWIN_CROSS_COMPILER:-clang}"
    cmd=(cargo xwin build)
  else
    echo "warning: unknown host OS; trying plain cargo build (may fail for ring/msvc)." >&2
    cmd=(cargo build)
  fi
  "${cmd[@]}" --release --target "$TARGET_TRIPLE" -p deploy-server -p control-api -p deploy-client
}

ARCH_RAW="${ARCH:-amd64}"
case "${ARCH_RAW,,}" in
  amd64|x86_64)
    TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-pc-windows-msvc}"
    STAGE_DIR_NAME="pirate-windows-amd64"
    OUT_PREFIX="pirate-windows-amd64"
    ;;
  arm64|aarch64)
    TARGET_TRIPLE="${TARGET_TRIPLE:-aarch64-pc-windows-msvc}"
    STAGE_DIR_NAME="pirate-windows-arm64"
    OUT_PREFIX="pirate-windows-arm64"
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
  OUT_ZIP="$DIST_DIR/${OUT_PREFIX}-no-ui-${REL}-${DATE_TAG}.zip"
else
  OUT_ZIP="$DIST_DIR/${OUT_PREFIX}-${REL}-${DATE_TAG}.zip"
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
  echo "==> frontend: skip (UI_BUILD=0)"
fi

echo "==> Rust release ($TARGET_TRIPLE)"
cargo_windows_release

BIN_DIR="$CARGO_TARGET_DIR/$TARGET_TRIPLE/release"
for b in deploy-server.exe control-api.exe client.exe pirate.exe; do
  if [[ ! -f "$BIN_DIR/$b" ]]; then
    echo "missing: $BIN_DIR/$b"
    exit 1
  fi
done

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/nginx" "$STAGE/lib/pirate"

cp -a "$BIN_DIR/deploy-server.exe" "$BIN_DIR/control-api.exe" "$BIN_DIR/client.exe" "$BIN_DIR/pirate.exe" "$STAGE/bin/"

if [[ "$UI_BUILD" == "1" ]]; then
  mkdir -p "$STAGE/share/ui/dist"
  cp -a "$REPO_ROOT/server-stack/frontend/dist/." "$STAGE/share/ui/dist/"
else
  : >"$STAGE/.bundle-no-ui"
fi

CONFIG_JSON="$REPO_ROOT/server-stack/deploy/windows/build-config.json"
UBUNTU_DIR="$REPO_ROOT/server-stack/deploy/ubuntu"
WIN_DEPLOY="$REPO_ROOT/server-stack/deploy/windows"

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

if command -v rsync >/dev/null 2>&1; then
  rsync -a --exclude "build-config.json" "$WIN_DEPLOY/" "$STAGE/"
else
  ( cd "$WIN_DEPLOY" && tar cf - --exclude=build-config.json . ) | ( cd "$STAGE" && tar xpf - )
fi

chmod +x "$REPO_ROOT/scripts/write-server-stack-manifest.sh"
"$REPO_ROOT/scripts/write-server-stack-manifest.sh" "$STAGE" "$TARGET_TRIPLE" "$REPO_ROOT" "$UI_BUILD"

rm -f "$OUT_ZIP"
mkdir -p "$DIST_DIR"
( cd "$DIST_DIR/stage" && zip -r -q "$OUT_ZIP" "$STAGE_DIR_NAME" )

echo ""
echo "Done: $OUT_ZIP"
echo "release=$REL target=$TARGET_TRIPLE"
