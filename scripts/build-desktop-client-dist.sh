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
#   DMG_README_TEMPLATE — path to README template for macos-dmg (default: scripts/dmg-bundle-README.in.txt;
#     placeholders: @VERSION@ @ARCH@ @DATE@ @PREFIX@ → README.txt at DMG root)
# Windows cross (Darwin/Linux): clang + cargo-xwin + NSIS (makensis). Real WiX .msi only on Windows;
#   windows-msi on Unix builds NSIS → dist/<prefix>-windows-<arch>-<ver>-<date>-nsis.zip
#   cargo-xwin defaults to clang-cl (/imsvc); we set XWIN_CROSS_COMPILER=clang before every cargo xwin
#   so C deps (e.g. ring) that invoke plain clang get compatible -I flags (see scripts/windows-bundle-build.sh).
#
# Linux .deb (linux-tgz) on macOS: Docker runs the same build inside Linux (dpkg-deb / Tauri deb).
#   Requires Docker; image: PIRATE_LINUX_DESKTOP_BUILD_IMAGE (default rust:bookworm).
#   Disable: PIRATE_LINUX_BUILD_NO_DOCKER=1 (then you need a Linux host).
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

# tauri-winres embeds the Windows icon/manifest via a .rc file; on non-Windows hosts it uses llvm-rc.
ensure_llvm_rc_for_win_cross() {
  if command -v llvm-rc >/dev/null 2>&1; then
    return 0
  fi
  local p
  for p in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin; do
    if [[ -x "$p/llvm-rc" ]]; then
      export PATH="$p:$PATH"
      return 0
    fi
  done
  echo "error: llvm-rc not in PATH — required for Windows resources (tauri-winres) when cross-building." >&2
  echo "  macOS: brew install llvm" >&2
  echo "         then: export PATH=\"/opt/homebrew/opt/llvm/bin:\$PATH\"   # Apple Silicon" >&2
  echo "               export PATH=\"/usr/local/opt/llvm/bin:\$PATH\"     # Intel" >&2
  echo "  Linux: sudo apt install llvm  (or llvm-N; ensure llvm-rc is on PATH)" >&2
  exit 1
}

prepare_windows_cross_nsis_build() {
  ensure_clang_for_client_win_cross
  ensure_llvm_rc_for_win_cross
  rustup component add llvm-tools >/dev/null 2>&1 || true
  ensure_cargo_xwin_client
  ensure_makensis_for_win_cross
  # cargo-xwin's default is clang-cl (MSVC /imsvc in CFLAGS). Crates like ring call plain `clang` for
  # aarch64-pc-windows-msvc; `/imsvc` is only valid for clang-cl. Apply before *every* `cargo xwin`
  # (pirate CLI build runs before tauri and previously missed this — ring failed with `/imsvc`).
  export XWIN_CROSS_COMPILER="${XWIN_CROSS_COMPILER:-clang}"
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

# Add README.txt at the root of a Tauri-produced DMG (read-only → UDRW → attach → copy → UDZO).
dmg_inject_readme() {
  local src_dmg="$1"
  local out_dmg="$2"
  local readme_template="${DMG_README_TEMPLATE:-$REPO_ROOT/scripts/dmg-bundle-README.in.txt}"
  local tmp_readme tmp_rw mntpnt

  if [[ ! -f "$readme_template" ]]; then
    echo "warning: DMG README template missing ($readme_template); copying .dmg unchanged." >&2
    cp -f "$src_dmg" "$out_dmg"
    return 0
  fi

  tmp_readme="$(mktemp "${TMPDIR:-/tmp}/pirate-dmg-readme.XXXXXX.txt")"
  tmp_rw="$(mktemp "${TMPDIR:-/tmp}/pirate-dmg-rw.XXXXXX.dmg")"
  mntpnt="$(mktemp -d "${TMPDIR:-/tmp}/pirate-dmg-mnt.XXXXXX")"
  rm -f "$tmp_rw"

  sed \
    -e "s|@VERSION@|$REL|g" \
    -e "s|@ARCH@|$ARCH_N|g" \
    -e "s|@DATE@|$DATE_TAG|g" \
    -e "s|@PREFIX@|$DIST_ARTIFACT_PREFIX|g" \
    "$readme_template" >"$tmp_readme"

  hdiutil convert "$src_dmg" -format UDRW -o "$tmp_rw"
  hdiutil attach "$tmp_rw" -readwrite -nobrowse -mountpoint "$mntpnt"
  cp -f "$tmp_readme" "$mntpnt/README.txt"
  sync
  hdiutil detach "$mntpnt"
  rm -f "$out_dmg"
  hdiutil convert "$tmp_rw" -format UDZO -imagekey zlib-level=9 -ov -o "$out_dmg"
  rm -f "$tmp_readme" "$tmp_rw"
  rmdir "$mntpnt" 2>/dev/null || true
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

# Same `pirate` CLI as `cargo build -p deploy-client --bin pirate` (local-stack/client).
pirate_cli_release_path() {
  local target="$1"
  if [[ "$target" == *-pc-windows-* ]]; then
    echo "$CARGO_TARGET_DIR/$target/release/pirate.exe"
  else
    echo "$CARGO_TARGET_DIR/$target/release/pirate"
  fi
}

build_pirate_cli_release() {
  local target="$1"
  echo "==> cargo build pirate CLI (deploy-client --bin pirate) target=$target"
  echo "    diag: CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>} PROFILE=release (must match next tauri/cargo step)"
  ( cd "$REPO_ROOT" && cargo build -p deploy-client --bin pirate --release --target "$target" )
  local bin
  bin="$(pirate_cli_release_path "$target")"
  if [[ -f "$bin" ]]; then
    echo "    diag: pirate CLI size $(wc -c <"$bin" | tr -d ' ') bytes at $bin"
  else
    echo "error: expected pirate binary missing: $bin" >&2
    exit 1
  fi
}

build_pirate_cli_release_win_cross() {
  local target="$1"
  echo "==> cargo xwin build pirate CLI (deploy-client --bin pirate) target=$target"
  ( cd "$REPO_ROOT" && cargo xwin build -p deploy-client --bin pirate --release --target "$target" )
}

stage_bundle_extra_pirate_for_deb() {
  local target="$1"
  local extra="$DESKTOP_UI/src-tauri/bundle-extra"
  local bin
  bin="$(pirate_cli_release_path "$target")"
  rm -rf "$extra"
  mkdir -p "$extra"
  cp -f "$bin" "$extra/pirate"
  chmod +x "$extra/pirate"
}

cleanup_bundle_extra() {
  rm -rf "$DESKTOP_UI/src-tauri/bundle-extra"
}

# Map host path under REPO_ROOT to /work/... for bind-mount builds.
host_path_to_work() {
  local abs="$1"
  local root="$2"
  if [[ "$abs" == "$root" ]]; then
    echo "/work"
    return 0
  fi
  if [[ "$abs" == "$root"/* ]]; then
    echo "/work/${abs#$root/}"
    return 0
  fi
  echo "error: path must be under REPO_ROOT ($root): $abs" >&2
  return 1
}

# Container platform matches target ARCH (arm64 → linux/arm64, amd64 → linux/amd64).
docker_linux_platform_for_arch() {
  case "$ARCH_N" in
    amd64) echo "linux/amd64" ;;
    arm64) echo "linux/arm64" ;;
  esac
}

# Run the Linux-native linux-tgz path inside Docker (macOS host).
linux_tgz_via_docker() {
  local platform image work_desktop host_desktop
  platform="$(docker_linux_platform_for_arch)"
  image="${PIRATE_LINUX_DESKTOP_BUILD_IMAGE:-rust:bookworm}"
  host_desktop="${DESKTOP_UI:-$REPO_ROOT/local-stack/desktop-ui}"
  work_desktop="$(host_path_to_work "$host_desktop" "$REPO_ROOT")" || exit 1

  if ! command -v docker >/dev/null 2>&1; then
    echo "error: Linux .deb bundle on macOS needs Docker (install Docker Desktop, or build on Linux)." >&2
    exit 1
  fi

  echo "==> Linux desktop build on macOS: Docker image=$image --platform=$platform (ARCH=$ARCH_N)"
  docker run --rm -i \
    --platform "$platform" \
    -v "$REPO_ROOT:/work" \
    -w /work \
    -e ARCH="$ARCH_N" \
    -e UI_BUILD="$UI_BUILD" \
    -e CARGO_TARGET_DIR=/work/target \
    -e DESKTOP_UI="$work_desktop" \
    -e DIST_ARTIFACT_PREFIX="$DIST_ARTIFACT_PREFIX" \
    -e WIN_EXE="$WIN_EXE" \
    "$image" \
    bash -s <<'DOCKER_BOOTSTRAP'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends \
  curl ca-certificates gnupg git \
  libwebkit2gtk-4.1-dev build-essential wget file \
  libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
  libxdo-dev \
  dpkg-dev \
  pkg-config
curl -fsSL https://deb.nodesource.com/setup_20.x | bash -
apt-get install -y --no-install-recommends nodejs
chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
./scripts/build-desktop-client-dist.sh linux-tgz
DOCKER_BOOTSTRAP
}

mkdir -p "$DIST_DIR"
export VITE_APP_RELEASE="$REL"

case "$MODE" in
  linux-tgz)
    if is_darwin && [[ -z "${PIRATE_LINUX_BUILD_NO_DOCKER:-}" ]]; then
      linux_tgz_via_docker
      exit 0
    fi
    is_linux || {
      echo "error: Linux .deb bundle — run on Linux (dpkg-deb / Tauri deb target), or on macOS with Docker (unset PIRATE_LINUX_BUILD_NO_DOCKER)." >&2
      exit 1
    }
    TARGET="$(rust_triple_linux)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    trap cleanup_bundle_extra EXIT
    build_pirate_cli_release "$TARGET"
    stage_bundle_extra_pirate_for_deb "$TARGET"
    echo "==> tauri build (deb) target=$TARGET version=$REL"
    run_tauri_build "$TARGET" --bundles deb
    cleanup_bundle_extra
    trap - EXIT
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
    echo "note: after dpkg -i, PATH includes pirate (CLI) and pirate-client (Tauri)."
    ;;

  macos-tgz)
    is_darwin || {
      echo "error: macOS .app bundle — run on Darwin (macOS)." >&2
      exit 1
    }
    TARGET="$(rust_triple_mac)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    build_pirate_cli_release "$TARGET"
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
    STAGE="$(mktemp -d "${TMPDIR:-/tmp}/pirate-client-macos-stage.XXXXXX")"
    trap 'rm -rf "$STAGE"' EXIT
    cp -R "$APP_PATH" "$STAGE/"
    mkdir -p "$STAGE/bin"
    cp -f "$(pirate_cli_release_path "$TARGET")" "$STAGE/bin/pirate"
    chmod +x "$STAGE/bin/pirate"
    tar -czf "$OUT_TGZ" -C "$STAGE" "$(basename "$APP_PATH")" bin
    rm -rf "$STAGE"
    trap - EXIT
    echo "Done: $OUT_TGZ"
    echo "note: add bin/ to PATH (or symlink bin/pirate) to run pirate in the terminal."
    ;;

  macos-dmg)
    is_darwin || {
      echo "error: macOS DMG — run on Darwin (macOS)." >&2
      exit 1
    }
    TARGET="$(rust_triple_mac)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    build_pirate_cli_release "$TARGET"
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
    dmg_inject_readme "$DMG_SRC" "$OUT_DMG"
    echo "Done: $OUT_DMG (README.txt added inside the disk image)"
    ;;

  windows-zip)
    TARGET="$(rust_triple_win)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    if is_windows_shell; then
      build_pirate_cli_release "$TARGET"
      echo "==> tauri build (nsis → zip) target=$TARGET version=$REL"
      run_tauri_build "$TARGET" --bundles nsis
    elif host_unix_for_win_cross; then
      prepare_windows_cross_nsis_build
      build_pirate_cli_release_win_cross "$TARGET"
      echo "==> tauri cross-build (cargo-xwin + NSIS → zip) target=$TARGET version=$REL"
      # Do not pass --bundles nsis: on macOS/Linux the CLI only lists host bundle kinds (ios/app/dmg or deb/appimage)
      # and rejects nsis before applying --target. Default bundle set for a Windows target still produces NSIS.
      run_tauri_build_win_cross "$TARGET"
    else
      echo "error: Windows installer ZIP — run on Windows, macOS, or Linux." >&2
      exit 1
    fi
    OUT_ZIP="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-windows-${ARCH_N}-${REL}-${DATE_TAG}.zip"
    package_win_nsis_or_exe_into_zip "$TARGET" "$OUT_ZIP"
    PIRATE_WIN="$(pirate_cli_release_path "$TARGET")"
    if [[ -f "$PIRATE_WIN" ]]; then
      zip -uj "$OUT_ZIP" "$PIRATE_WIN"
      echo "note: zip also contains pirate.exe (CLI); add its folder to PATH or copy next to the installer output."
    fi
    echo "Done: $OUT_ZIP"
    ;;

  windows-msi)
    TARGET="$(rust_triple_win)"
    rustup_target_add "$TARGET"
    ensure_npm_deps
    if is_windows_shell; then
      build_pirate_cli_release "$TARGET"
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
      PIRATE_WIN="$(pirate_cli_release_path "$TARGET")"
      if [[ -f "$PIRATE_WIN" ]]; then
        OUT_PIRATE_EXE="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-pirate-cli-windows-${ARCH_N}-${REL}-${DATE_TAG}.exe"
        cp -f "$PIRATE_WIN" "$OUT_PIRATE_EXE"
        echo "note: pirate CLI (for PATH): $OUT_PIRATE_EXE"
      fi
      echo "Done: $OUT_MSI"
    elif host_unix_for_win_cross; then
      echo "note: WiX .msi is only produced on Windows (WiX Toolset)." >&2
      echo "      On this host we build the NSIS installer via cargo-xwin instead." >&2
      prepare_windows_cross_nsis_build
      build_pirate_cli_release_win_cross "$TARGET"
      run_tauri_build_win_cross "$TARGET"
      OUT_ZIP="$DIST_DIR/${DIST_ARTIFACT_PREFIX}-windows-${ARCH_N}-${REL}-${DATE_TAG}-nsis.zip"
      package_win_nsis_or_exe_into_zip "$TARGET" "$OUT_ZIP"
      PIRATE_WIN="$(pirate_cli_release_path "$TARGET")"
      if [[ -f "$PIRATE_WIN" ]]; then
        zip -uj "$OUT_ZIP" "$PIRATE_WIN"
      fi
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
