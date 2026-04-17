#!/usr/bin/env bash
# OTA: upload a Linux server-stack .tar.gz to deploy-server via `pirate update`.
#
# Usage:
#   ./scripts/pirate-server-stack-update.sh <grpc_url> [path/to/bundle.tar.gz]
#
# If the bundle path is omitted, picks the newest matching file in dist/:
#   ARCH=arm64  → pirate-linux-aarch64-no-ui-*.tar.gz (fallback: pirate-linux-aarch64-*.tar.gz)
#   ARCH=amd64  → pirate-linux-amd64-no-ui-*.tar.gz   (fallback: pirate-linux-amd64-*.tar.gz)
#
# Env:
#   PIRATE_BIN  — path to `pirate` (default: `pirate` on PATH, else `cargo run -p deploy-client --bin pirate`)
#   ARCH        — amd64 | arm64 (default: amd64)
#   REPO_ROOT   — repo root (default: parent of scripts/)
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
URL="${1:?usage: $0 <grpc_url> [bundle.tar.gz]}"
BUNDLE="${2:-}"

ARCH="${ARCH:-amd64}"
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

if [[ -z "$BUNDLE" ]]; then
  BUNDLE="$(pick_bundle)"
fi
if [[ -z "$BUNDLE" || ! -f "$BUNDLE" ]]; then
  echo "error: no bundle found. Build first, e.g.:" >&2
  echo "  make dist-linux ARCH=$ARCH UI_BUILD=0" >&2
  exit 1
fi

run_pirate() {
  if [[ -n "${PIRATE_BIN:-}" ]]; then
    exec "$PIRATE_BIN" "$@"
  fi
  if command -v pirate >/dev/null 2>&1; then
    exec pirate "$@"
  fi
  cd "$REPO_ROOT"
  exec cargo run --release -p deploy-client --bin pirate -- "$@"
}

echo "bundle: $BUNDLE"
echo "url:    $URL"
run_pirate update "$BUNDLE" --url "$URL"
