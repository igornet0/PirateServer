#!/usr/bin/env bash
# Linux aarch64 bundle — delegates to linux-bundle-build.sh arm64
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$ROOT/scripts/linux-bundle-build.sh" arm64
