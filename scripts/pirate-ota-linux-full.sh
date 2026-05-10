#!/usr/bin/env bash
# One-shot: build Linux server bundle, then `pirate update` to deploy-server (OTA).
#
# Usage:
#   ./scripts/pirate-ota-linux-full.sh <grpc_http_url> [path/to/bundle.tar.gz]
#
# Examples:
#   ./scripts/pirate-ota-linux-full.sh http://192.168.0.30:50051
#   ARCH=arm64 UI_BUILD=0 ./scripts/pirate-ota-linux-full.sh http://192.168.0.30:50051
#
# Env:
#   ARCH   — amd64 | arm64 (default: amd64)
#   UI_BUILD — 1 = include dashboard static + Tauri desktop client in bundle (default: 1)
#   REPO_ROOT — repo root (default: parent of scripts/)
#   PIRATE_BIN — path to `pirate` binary (default: `pirate` on PATH, else `cargo run ...`)
#
# Non-interactive `pirate update` (no TTY):
#   When the host has no UI yet and the bundle includes UI, deploy-client reads
#   PIRATE_UPDATE_* from the environment — set them before running this script, e.g.:
#     export PIRATE_UPDATE_DEPLOY_ALLOW_SERVER_STACK_UPDATE=1
#     export PIRATE_UPDATE_INSTALL_NGINX=0
#     export PIRATE_UPDATE_DOMAIN=                         # empty = IP from host, see stack_update_prompt.rs
#   Stdin is redirected from /dev/null so prompts are not used.
#
# After OTA, the server runs /usr/local/lib/pirate/pirate-ensure-file-storage.sh (root) to create
# PIRATE_STORAGE_ROOT and .pirate-tmp for the desktop file manager — no manual mkdir.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
URL="${1:?usage: $0 <grpc_http_url> [bundle.tar.gz]}"
BUNDLE="${2:-}"

ARCH="${ARCH:-amd64}"
UI_BUILD="${UI_BUILD:-1}"

case "${ARCH,,}" in
arm64|aarch64)
  PFX="pirate-linux-aarch64"
  ;;
amd64|x86_64)
  PFX="pirate-linux-amd64"
  ;;
*)
  echo "error: ARCH=$ARCH — use amd64 or arm64" >&2
  exit 1
  ;;
esac

pick_bundle() {
  shopt -s nullglob
  local -a nui=( "$REPO_ROOT/dist/${PFX}-no-ui-"*.tar.gz )
  if ((${#nui[@]})); then
    ls -t "${nui[@]}" | head -1
    return 0
  fi
  local -a all=( "$REPO_ROOT/dist/${PFX}-"*.tar.gz )
  local -a plain=()
  for f in "${all[@]}"; do
    [[ "$f" == *"-no-ui-"* ]] && continue
    plain+=("$f")
  done
  if ((${#plain[@]})); then
    ls -t "${plain[@]}" | head -1
    return 0
  fi
  echo ""
}

cd "$REPO_ROOT"
echo "==> make dist-linux ARCH=$ARCH UI_BUILD=$UI_BUILD"
make dist-linux "ARCH=$ARCH" "UI_BUILD=$UI_BUILD"

if [[ -z "$BUNDLE" ]]; then
  BUNDLE="$(pick_bundle)"
fi
if [[ -z "$BUNDLE" || ! -f "$BUNDLE" ]]; then
  echo "error: no bundle in dist/. Build failed or wrong ARCH." >&2
  exit 1
fi

run_pirate() {
  if [[ -n "${PIRATE_BIN:-}" ]]; then
    "$PIRATE_BIN" "$@"
    return
  fi
  if command -v pirate >/dev/null 2>&1; then
    pirate "$@"
    return
  fi
  cargo run --release -p deploy-client --bin pirate -- "$@"
}

echo "bundle: $BUNDLE"
echo "url:    $URL"
# No TTY: use env for enable_ui transition (see local-stack/client/src/stack_update_prompt.rs)
exec </dev/null
run_pirate update "$BUNDLE" --url "$URL"
