#!/usr/bin/env bash
# Linux bundle — delegates to linux-bundle-build.sh (default ARCH=amd64).
# Usage: ARCH=amd64|arm64 UI_BUILD=0|1 ./scripts/build-linux-bundle.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCH="${ARCH:-amd64}"
exec "$ROOT/scripts/linux-bundle-build.sh" "$ARCH"
