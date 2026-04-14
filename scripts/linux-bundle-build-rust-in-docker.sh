#!/usr/bin/env bash
# Run Rust release build for Linux bundle inside Docker (rust:bookworm). Use from macOS when
# cargo-zigbuild cannot link deploy-client (xcap → libxcb). See linux-bundle-build.sh.
#
# Usage: ./scripts/linux-bundle-build-rust-in-docker.sh <TARGET_TRIPLE>
# Env: REPO_ROOT (optional), CARGO_TARGET_DIR (optional, default REPO_ROOT/target)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
TARGET_TRIPLE="${1:?usage: $0 <TARGET_TRIPLE>}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

case "$TARGET_TRIPLE" in
  aarch64-unknown-linux-gnu) DOCKER_PLATFORM=linux/arm64 ;;
  x86_64-unknown-linux-gnu) DOCKER_PLATFORM=linux/amd64 ;;
  *)
    echo "$0: unsupported TARGET_TRIPLE=$TARGET_TRIPLE" >&2
    exit 1
    ;;
esac

if ! command -v docker >/dev/null 2>&1; then
  echo "$0: docker not found" >&2
  exit 1
fi

INNER_WORK=/work
DOCKER_RUN=(docker run --rm --platform "$DOCKER_PLATFORM")

# Repo sources
DOCKER_RUN+=(-v "$REPO_ROOT:$INNER_WORK")

# Optional separate target dir (same logic as host). Normalize paths so symlinked repos
# (e.g. /var vs /private/var on macOS) still use /work/target inside the container.
INNER_CARGO_TARGET_DIR="$INNER_WORK/target"
_default_target="$REPO_ROOT/target"
if command -v python3 >/dev/null 2>&1; then
  _ct="$(python3 -c 'import os,sys; print(os.path.realpath(os.path.expanduser(sys.argv[1])))' "$CARGO_TARGET_DIR")"
  _dt="$(python3 -c 'import os,sys; print(os.path.realpath(os.path.expanduser(sys.argv[1])))' "$_default_target")"
else
  _ct="$CARGO_TARGET_DIR"
  _dt="$_default_target"
fi
if [[ "$_ct" != "$_dt" ]]; then
  DOCKER_RUN+=(-v "$CARGO_TARGET_DIR:/cargo-target")
  INNER_CARGO_TARGET_DIR=/cargo-target
fi

DOCKER_RUN+=(
  -e "TARGET_TRIPLE=$TARGET_TRIPLE"
  -e "CARGO_TARGET_DIR=$INNER_CARGO_TARGET_DIR"
  -v "$SCRIPT_DIR/linux-bundle-rust-docker-entry.sh:/entry.sh:ro"
  rust:bookworm
  bash /entry.sh
)

echo "==> docker: platform=$DOCKER_PLATFORM target=$TARGET_TRIPLE CARGO_TARGET_DIR=$INNER_CARGO_TARGET_DIR"
"${DOCKER_RUN[@]}"
