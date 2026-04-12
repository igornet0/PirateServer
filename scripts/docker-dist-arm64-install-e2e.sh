#!/usr/bin/env bash
# Build aarch64 dist (unless SKIP_DIST_BUILD=1), build arm64 test image, run install.sh + client auth/version inside Docker.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

IMAGE="${DIST_ARM64_INSTALL_E2E_IMAGE:-pirate-dist-arm64-install-e2e}"

if [[ "${SKIP_DIST_BUILD:-}" != "1" ]]; then
  echo "==> make dist-arm64-linux (set SKIP_DIST_BUILD=1 to reuse existing dist/*.tar.gz)"
  make dist-arm64-linux
else
  echo "==> SKIP_DIST_BUILD=1 — using existing dist/pirate-linux-aarch64-*.tar.gz"
fi

shopt -s nullglob
bundles=(dist/pirate-linux-aarch64-*.tar.gz)
if [[ ${#bundles[@]} -eq 0 ]]; then
  echo "error: no dist/pirate-linux-aarch64-*.tar.gz — run: make dist-arm64-linux" >&2
  exit 1
fi

echo "==> docker build ($IMAGE, linux/arm64)"
if docker buildx version >/dev/null 2>&1; then
  docker buildx build --platform linux/arm64 \
    -f scripts/Dockerfile.dist-arm64-install-e2e \
    -t "$IMAGE" \
    --load \
    "$ROOT"
else
  docker build --platform linux/arm64 \
    -f scripts/Dockerfile.dist-arm64-install-e2e \
    -t "$IMAGE" \
    "$ROOT"
fi

echo "==> docker run (mount dist/)"
docker run --rm --platform linux/arm64 \
  -v "$ROOT/dist:/dist:ro" \
  "$IMAGE"
