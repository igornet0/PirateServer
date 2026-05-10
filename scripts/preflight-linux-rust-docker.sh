#!/usr/bin/env bash
# Check that `rustc` can run in rust:bookworm for the given Linux target. Used from
# linux-bundle-build.sh (before npm) and linux-bundle-build-rust-in-docker.sh
# to fail fast; on Apple Silicon, linux/amd64 often cannot run the Rust host toolchain.
# Usage: DOCKER_RUST_IMAGE=... ./scripts/preflight-linux-rust-docker.sh <TARGET_TRIPLE>
set -euo pipefail

TARGET_TRIPLE="${1:?usage: $0 <TARGET_TRIPLE>}"
DOCKER_RUST_IMAGE="${DOCKER_RUST_IMAGE:-rust:bookworm}"

case "$TARGET_TRIPLE" in
  aarch64-unknown-linux-gnu) DOCKER_PLATFORM=linux/arm64 ;;
  x86_64-unknown-linux-gnu) DOCKER_PLATFORM=linux/amd64 ;;
  *)
    echo "preflight-linux-rust-docker: unsupported target $TARGET_TRIPLE" >&2
    exit 1
    ;;
esac

if ! command -v docker >/dev/null 2>&1; then
  echo "preflight-linux-rust-docker: docker not found" >&2
  exit 1
fi

echo "==> docker preflight: $DOCKER_RUST_IMAGE ($DOCKER_PLATFORM) rustc -vV"
if docker run --rm --platform "$DOCKER_PLATFORM" \
  -e "DOCKER_RUST_IMAGE=$DOCKER_RUST_IMAGE" \
  "$DOCKER_RUST_IMAGE" \
  bash -c 'command -v rustc >/dev/null && exec rustc -vV'; then
  exit 0
fi

echo "preflight-linux-rust-docker: rustc will not run in this Docker + platform combination (broken emulation or image)." >&2
if [[ "$(uname -m)" == "arm64" && "$DOCKER_PLATFORM" == "linux/amd64" ]]; then
  echo "On Apple Silicon, user-mode (QEMU) or half-broken amd64 paths break rustc (e.g. librustc_driver: no loadable segments). «Use Rosetta for x86/amd64» in Docker Desktop often does not fix it for rustc — that is expected." >&2
  echo "Reliable approach: Colima with an x86_64 Linux VM (linux/amd64 is native there). Lima 2.1+ needs guest agents for x86_64 (without them colima start --arch x86_64 fails):" >&2
  echo "  brew install colima lima-additional-guestagents" >&2
  echo "  colima stop" >&2
  echo "  colima start --arch x86_64" >&2
  echo "  # If you still see «guest agent ... Linux-x86_64» / VM start error:  brew reinstall lima-additional-guestagents" >&2
  echo "  # then reset the broken profile:  colima delete  &&  colima start --arch x86_64" >&2
  echo "  docker context use colima" >&2
  echo "  make dist-linux ARCH=amd64 UI_BUILD=1" >&2
  echo "Alternatives: x86_64 Linux host or CI, or ARCH=arm64 if the server is ARM64." >&2
else
  echo "Try:  DOCKER_RUST_IMAGE_PULL=1 make dist-linux   or   DOCKER_RUST_IMAGE=rust:1.88-bookworm make dist-linux" >&2
fi
exit 1
