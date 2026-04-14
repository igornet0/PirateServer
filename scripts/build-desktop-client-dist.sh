#!/usr/bin/env bash
# Release artifacts for Tauri desktop bundles (default: local-stack/desktop-ui → pirate-client).
# Invoked from Makefile: dist-client-* | dist-desktop-*
#
# Usage: ./scripts/build-desktop-client-dist.sh linux-tgz|macos-tgz|macos-dmg|windows-zip|windows-msi
# Env:
#   ARCH=amd64|arm64 (default amd64)
#   UI_BUILD=0|1 (ignored for Tauri; embedded UI is always built)
#   DESKTOP_UI — absolute path to Tauri app root (default: $REPO_ROOT/local-stack/desktop-ui).
#     Deploy dashboard: DESKTOP_UI=$REPO_ROOT/server-stack/desktop-ui
#   DIST_ARTIFACT_PREFIX — output basename prefix (default: pirate-client), e.g. deploy-dashboard-desktop
#   WIN_EXE — Windows exe name for NSIS fallback zip (default: pirate-client.exe), e.g. deploy-dashboard-desktop.exe
# Windows cross (Darwin/Linux): clang + cargo-xwin + NSIS (makensis). Real WiX .msi only on Windows;
#   windows-msi on Unix builds NSIS → dist/<prefix>-windows-<arch>-<ver>-<date>-nsis.zip
#
set -euo pipefail

MODE="${1:?usage: $0 linux-tgz|macos-tgz|macos-dmg|windows-zip|windows-msi}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

ARCH_RAW="${ARCH:-amd64}"
case "${ARCH_RAW,,}" in
  amd64|x86_64) ARCH_N=amd64 ;;
  arm64|aarch64) ARCH_N=arm64 ;;
  *)
    echo "error: ARCH=$ARCH_RAW — expected amd64 or arm64" >&2
    exit 1
    ;;
esac

UI_BUILD="${UI_BUILD:-1}"
case "${UI_BUILD,,}" in
  0|false|no|off)
    echo "note: UI_BUILD=0 — desktop client still bundles the local UI (Tauri); flag ignored." >&2
    ;;
esac

REL="$("$REPO_ROOT/scripts/read-version.sh")"
DATE_TAG="$(date +%Y%m%d)"
DIST_DIR="$REPO_ROOT/dist"
DESKTOP_UI="${DESKTOP_UI:-$REPO_ROOT/local-stack/desktop-ui}"
DIST_ARTIFACT_PREFIX="${DIST_ARTIFACT_PREFIX:-pirate-client}"
WIN_EXE="${WIN_EXE:-pirate-client.exe}"

rust_triple_mac() {
  case "$ARCH_N" in
    amd64) echo "x86_64-apple-darwin" ;;
    arm64) echo "aarch64-apple-darwin" ;;
  esac
}

rust_triple_win() {
  case "$ARCH_N" in
    amd64) echo "x86_64-pc-windows-msvc" ;;
    arm64) echo "aarch64-pc-windows-msvc" ;;
  esac
}

rust_triple_linux() {
  case "$ARCH_N" in
    amd64) echo "x86_64-unknown-linux-gnu" ;;
    arm64) echo "aarch64-unknown-linux-gnu" ;;
  esac
}

is_darwin() { [[ "$(uname -s)" == "Darwin" ]]; }

is_linux() { [[ "$(uname -s)" == "Linux" ]]; }

is_windows_shell() {
  case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) return 0 ;;
  esac
  return 1
}

host_unix_for_win_cross() {
  is_darwin || is_linux
}

ensure_clang_for_client_win_cross() {
  if command -v clang >/dev/null 2>&1; then
    return 0
  fi
  echo "error: clang not in PATH (required for cargo-xwin Windows cross-build)." >&2
  echo "  macOS: xcode-select --install  or  brew install llvm" >&2
  echo "  Linux: sudo apt install clang" >&2
  exit 1
}

ensure_cargo_xwin_client() {
  if cargo xwin --help >/dev/null 2>&1; then
    return 0
  fi
  if [[ -n "${PIRATE_SKIP_CARGO_XWIN_INSTALL:-}" ]]; then
    echo "error: cargo-xwin not found; install: cargo install --locked cargo-xwin" >&2
    echo "  (or unset PIRATE_SKIP_CARGO_XWIN_INSTALL for auto-install)" >&2
    exit 1
  fi
  echo "==> installing cargo-xwin (Windows MSVC SDK; Microsoft license applies)"
  cargo install --locked cargo-xwin
}

ensure_makensis_for_win_cross() {
  if command -v makensis >/dev/null 2>&1; then
    return 0
  fi
  echo "error: makensis (NSIS) not in PATH — required for Windows installer cross-build." >&2
  echo "  macOS: brew install nsis" >&2
  echo "  Linux: sudo apt install nsis  (or your distro's nsis package)" >&2
  exit 1
}

prepare_windows_cross_nsis_build() {
  ensure_clang_for_client_win_cross
  rustup component add llvm-tools >/dev/null 2>&1 || true
  ensure_cargo_xwin_client
  ensure_makensis_for_win_cross
}

run_tauri_build_win_cross() {
  local target="$1"
  shift
  export XWIN_CROSS_COMPILER="${XWIN_CROSS_COMPILER:-clang}"
  ( cd "$DESKTOP_UI" && npx tauri build --ci --runner cargo-xwin --target "$target" --config "$(tauri_cfg_version)" "$@" )
}

package_win_nsis_or_exe_into_zip() {
  local TARGET="$1"
  local OUT_ZIP="$2"
  local NSIS_DIR SETUP
  NSIS_DIR="$(bundle_win_nsis_dir "$TARGET")"
  SETUP="$(find "$NSIS_DIR" -maxdepth 1 -name '*setup.exe' -print | head -n 1 || true)"
  rm -f "$OUT_ZIP"
  if [[ -n "$SETUP" && -f "$SETUP" ]]; then
    ( cd "$NSIS_DIR" && zip -q "$OUT_ZIP" "$(basename "$SETUP")" )
  else
    local BIN_DIR="$CARGO_TARGET_DIR/$TARGET/release"
    if [[ ! -f "$BIN_DIR/$WIN_EXE" ]]; then
      echo "error: missing NSIS setup under $NSIS_DIR and no $BIN_DIR/$WIN_EXE" >&2
      exit 1
    fi
    (
      cd "$BIN_DIR"
      zip -q "$OUT_ZIP" "$WIN_EXE" *.dll 2>/dev/null || zip -q "$OUT_ZIP" "$WIN_EXE"
    )
  fi
}

tauri_cfg_version() {
  # Single-line JSON for `tauri build --config` (align bundle version with repo VERSION).
  printf '{"version":"%s"}' "$REL"
}

run_tauri_build() {
  local target="$1"
  shift
  ( cd "$DESKTOP_UI" && npx tauri build --ci --target "$target" --config "$(tauri_cfg_version)" "$@" )
}

ensure_npm_deps() {
  (
    cd "$DESKTOP_UI"
    if [[ -f package-lock.json ]]; then
      npm ci
    else
      npm install
    fi
  )
}

rustup_target_add() {
  local t="$1"
  rustup target add "$t" >/dev/null 2>&1 || true
}

bundle_macos_app_dir() {
  local target="$1"
  echo "$CARGO_TARGET_DIR/$target/release/bundle/macos"
}

bundle_macos_dmg_dir() {
  local target="$1"
  echo "$CARGO_TARGET_DIR/$target/release/bundle/dmg"
}

bundle_win_nsis_dir() {
  local target="$1"
  echo "$CARGO_TARGET_DIR/$target/release/bundle/nsis"
}

bundle_win_msi_dir() {
  local target="$1"
  echo "$CARGO_TARGET_DIR/$target/release/bundle/msi"
}

bundle_linux_deb_dir() {
  local target="$1"
  echo "$CARGO_TARGET_DIR/$target/release/bundle/deb"
}

mkdir -p "$DIST_DIR"
export VITE_APP_RELEASE="$REL"

case "$MODE" in
  linux-tgz)
    is_linux || {
      echo "error: Linux .deb bundle — run on Linux (dpkg-deb / Tauri deb target)." >&2
      exit 1
    }
    TARGET="$(rust_triple_linux)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    echo "==> tauri build (deb) target=$TARGET version=$REL"
    run_tauri_build "$TARGET" --bundles deb
    DEB_DIR="$(bundle_linux_deb_dir "$TARGET")"
    DEB_PATH="$(find "$DEB_DIR" -maxdepth 1 -name '*.deb' -print | head -n 1 || true)"
    if [[ -z "$DEB_PATH" || ! -f "$DEB_PATH" ]]; then
      echo "error: no .deb under $DEB_DIR" >&2
      exit 1
    fi
    OUT_TGZ="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-linux-${ARCH_N}-${REL}-${DATE_TAG}.tar.gz"
    rm -f "$OUT_TGZ"
    tar -czf "$OUT_TGZ" -C "$(dirname "$DEB_PATH")" "$(basename "$DEB_PATH")"
    echo "Done: $OUT_TGZ"
    ;;

  macos-tgz)
    is_darwin || {
      echo "error: macOS .app bundle — run on Darwin (macOS)." >&2
      exit 1
    }
    TARGET="$(rust_triple_mac)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    echo "==> tauri build (app) target=$TARGET version=$REL"
    run_tauri_build "$TARGET" --bundles app
    MAC_DIR="$(bundle_macos_app_dir "$TARGET")"
    APP_PATH="$(find "$MAC_DIR" -maxdepth 1 -name '*.app' -print | head -n 1 || true)"
    if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
      echo "error: no .app under $MAC_DIR" >&2
      exit 1
    fi
    OUT_TGZ="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-macos-${ARCH_N}-${REL}-${DATE_TAG}.tar.gz"
    rm -f "$OUT_TGZ"
    export COPYFILE_DISABLE=1
    tar -czf "$OUT_TGZ" -C "$(dirname "$APP_PATH")" "$(basename "$APP_PATH")"
    echo "Done: $OUT_TGZ"
    ;;

  macos-dmg)
    is_darwin || {
      echo "error: macOS DMG — run on Darwin (macOS)." >&2
      exit 1
    }
    TARGET="$(rust_triple_mac)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    echo "==> tauri build (dmg) target=$TARGET version=$REL"
    run_tauri_build "$TARGET" --bundles dmg
    DMG_DIR="$(bundle_macos_dmg_dir "$TARGET")"
    DMG_SRC="$(find "$DMG_DIR" -maxdepth 1 -name '*.dmg' -print | head -n 1 || true)"
    if [[ -z "$DMG_SRC" || ! -f "$DMG_SRC" ]]; then
      echo "error: no .dmg under $DMG_DIR" >&2
      exit 1
    fi
    OUT_DMG="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-macos-${ARCH_N}-${REL}-${DATE_TAG}.dmg"
    rm -f "$OUT_DMG"
    cp -f "$DMG_SRC" "$OUT_DMG"
    echo "Done: $OUT_DMG"
    ;;

  windows-zip)
    TARGET="$(rust_triple_win)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    if is_windows_shell; then
      echo "==> tauri build (nsis → zip) target=$TARGET version=$REL"
      run_tauri_build "$TARGET" --bundles nsis
    elif host_unix_for_win_cross; then
      echo "==> tauri cross-build (cargo-xwin + NSIS → zip) target=$TARGET version=$REL"
      prepare_windows_cross_nsis_build
      run_tauri_build_win_cross "$TARGET" --bundles nsis
    else
      echo "error: Windows installer ZIP — run on Windows, macOS, or Linux." >&2
      exit 1
    fi
    OUT_ZIP="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-windows-${ARCH_N}-${REL}-${DATE_TAG}.zip"
    package_win_nsis_or_exe_into_zip "$TARGET" "$OUT_ZIP"
    echo "Done: $OUT_ZIP"
    ;;

  windows-msi)
    TARGET="$(rust_triple_win)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    if is_windows_shell; then
      echo "==> tauri build (msi) target=$TARGET version=$REL"
      run_tauri_build "$TARGET" --bundles msi
      MSI_DIR="$(bundle_win_msi_dir "$TARGET")"
      MSI_SRC="$(find "$MSI_DIR" -maxdepth 1 -name '*.msi' -print | head -n 1 || true)"
      if [[ -z "$MSI_SRC" || ! -f "$MSI_SRC" ]]; then
        echo "error: no .msi under $MSI_DIR" >&2
        exit 1
      fi
      OUT_MSI="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-windows-${ARCH_N}-${REL}-${DATE_TAG}.msi"
      rm -f "$OUT_MSI"
      cp -f "$MSI_SRC" "$OUT_MSI"
      echo "Done: $OUT_MSI"
    elif host_unix_for_win_cross; then
      echo "note: WiX .msi is only produced on Windows (WiX Toolset)." >&2
      echo "      On this host we build the NSIS installer via cargo-xwin instead." >&2
      prepare_windows_cross_nsis_build
      run_tauri_build_win_cross "$TARGET" --bundles nsis
      OUT_ZIP="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-windows-${ARCH_N}-${REL}-${DATE_TAG}-nsis.zip"
      package_win_nsis_or_exe_into_zip "$TARGET" "$OUT_ZIP"
      echo "Done (NSIS zip, not MSI): $OUT_ZIP"
    else
      echo "error: windows-msi — unsupported host OS" >&2
      exit 1
    fi
    ;;

  *)
    echo "usage: $0 linux-tgz|macos-tgz|macos-dmg|windows-zip|windows-msi" >&2
    exit 1
    ;;
esac
