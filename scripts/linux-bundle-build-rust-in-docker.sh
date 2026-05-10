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
CACHE_ROOT="${LINUX_BUNDLE_DOCKER_CACHE_DIR:-$REPO_ROOT/.cache/linux-bundle-docker}"
CARGO_HOME_CACHE_DIR="$CACHE_ROOT/cargo-home"
APT_CACHE_VOLUME="${LINUX_BUNDLE_DOCKER_APT_CACHE_VOLUME:-pirate-linux-bundle-apt-cache}"
APT_LISTS_VOLUME="${LINUX_BUNDLE_DOCKER_APT_LISTS_VOLUME:-pirate-linux-bundle-apt-lists}"

case "$TARGET_TRIPLE" in
  aarch64-unknown-linux-gnu) DOCKER_PLATFORM=linux/arm64 ;;
  x86_64-unknown-linux-gnu) DOCKER_PLATFORM=linux/amd64 ;;
  *)
    echo "$0: unsupported TARGET_TRIPLE=$TARGET_TRIPLE" >&2
    exit 1
    ;;
esac

_HOST_ARCH="$(uname -m)"
HOST_DOCKER_PLATFORM="linux/amd64"
if [[ "$_HOST_ARCH" == "arm64" || "$_HOST_ARCH" == "aarch64" ]]; then
  HOST_DOCKER_PLATFORM="linux/arm64"
fi
DOCKER_SERVER_ARCH="$(docker version --format '{{.Server.Arch}}' 2>/dev/null || true)"
DOCKER_SERVER_PLATFORM=""
case "$DOCKER_SERVER_ARCH" in
  amd64|x86_64) DOCKER_SERVER_PLATFORM="linux/amd64" ;;
  arm64|aarch64) DOCKER_SERVER_PLATFORM="linux/arm64" ;;
esac
if [[ "$_HOST_ARCH" == "arm64" && "$DOCKER_PLATFORM" == "linux/amd64" && "${PIRATE_RUST_IN_DOCKER_PREFLIGHT_OK:-}" != "1" ]]; then
  echo "note: Apple Silicon + linux/amd64: if the next build fails, use Colima (see scripts/preflight-linux-rust-docker.sh) or build on x86_64/CI." >&2
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "$0: docker not found" >&2
  exit 1
fi

mkdir -p "$CARGO_HOME_CACHE_DIR"
docker volume create "$APT_CACHE_VOLUME" >/dev/null
docker volume create "$APT_LISTS_VOLUME" >/dev/null

# Default rust:bookworm (moving tag). Override if needed, e.g. DOCKER_RUST_IMAGE=rust:1.88-bookworm
DOCKER_RUST_IMAGE="${DOCKER_RUST_IMAGE:-rust:bookworm}"
# Set DOCKER_RUST_IMAGE_PULL=1 to `docker pull` before run (fixes stale/corrupt layers: rustc/librustc_driver "no loadable segments").
if [[ "${DOCKER_RUST_IMAGE_PULL:-0}" == "1" ]]; then
  echo "==> docker pull $DOCKER_RUST_IMAGE"
  docker pull "$DOCKER_RUST_IMAGE"
fi

_can_run_rust_image() {
  local platform="$1"
  docker run --rm --platform "$platform" \
    "$DOCKER_RUST_IMAGE" \
    bash -c 'command -v rustc >/dev/null && exec rustc -vV' >/dev/null 2>&1
}

if [[ "${PIRATE_RUST_IN_DOCKER_PREFLIGHT_OK:-}" == "1" ]]; then
  :
else
  chmod +x "$SCRIPT_DIR/preflight-linux-rust-docker.sh" 2>/dev/null || true
  if ! DOCKER_RUST_IMAGE="$DOCKER_RUST_IMAGE" \
      "$SCRIPT_DIR/preflight-linux-rust-docker.sh" "$TARGET_TRIPLE"; then
    fallback_candidates=()
    if [[ -n "$DOCKER_SERVER_PLATFORM" ]]; then
      fallback_candidates+=("$DOCKER_SERVER_PLATFORM")
    fi
    fallback_candidates+=("$HOST_DOCKER_PLATFORM")
    fallback_candidates+=("linux/amd64" "linux/arm64")

    selected_platform=""
    for candidate in "${fallback_candidates[@]}"; do
      [[ -n "$candidate" ]] || continue
      if [[ "$candidate" == "$DOCKER_PLATFORM" ]]; then
        continue
      fi
      if _can_run_rust_image "$candidate"; then
        selected_platform="$candidate"
        break
      fi
    done

    if [[ -z "$selected_platform" ]]; then
      echo "$0: no runnable docker platform found for image $DOCKER_RUST_IMAGE (tried target=$DOCKER_PLATFORM, server=${DOCKER_SERVER_PLATFORM:-unknown}, host=$HOST_DOCKER_PLATFORM)." >&2
      exit 1
    fi

    echo "warning: requested docker platform $DOCKER_PLATFORM cannot execute rust image; using fallback $selected_platform with cross-linker setup for $TARGET_TRIPLE." >&2
    DOCKER_PLATFORM="$selected_platform"
    export PIRATE_DOCKER_CROSS_FALLBACK=1
  fi
fi

INNER_WORK=/work
DOCKER_RUN=(
  docker run --rm
  --platform "$DOCKER_PLATFORM"
  -e "DOCKER_RUST_IMAGE=$DOCKER_RUST_IMAGE"
  -e "HOST_MACHINE=$(uname -m)"
  -e "CARGO_HOME=/cargo-home"
  -e "CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse"
  -e "CARGO_TERM_COLOR=always"
  -e "APT_CACHE_DIR=/var/cache/apt"
  -e "APT_LISTS_DIR=/var/lib/apt/lists"
)

# Repo sources
DOCKER_RUN+=(-v "$REPO_ROOT:$INNER_WORK")
DOCKER_RUN+=(-v "$CARGO_HOME_CACHE_DIR:/cargo-home")
DOCKER_RUN+=(-v "$APT_CACHE_VOLUME:/var/cache/apt")
DOCKER_RUN+=(-v "$APT_LISTS_VOLUME:/var/lib/apt/lists")

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
  -e "PIRATE_DOCKER_CROSS_FALLBACK=${PIRATE_DOCKER_CROSS_FALLBACK:-0}"
  -v "$SCRIPT_DIR/linux-bundle-rust-docker-entry.sh:/entry.sh:ro"
  "$DOCKER_RUST_IMAGE"
  bash /entry.sh
)

echo "==> docker: image=$DOCKER_RUST_IMAGE platform=$DOCKER_PLATFORM target=$TARGET_TRIPLE CARGO_TARGET_DIR=$INNER_CARGO_TARGET_DIR"
"${DOCKER_RUN[@]}"
