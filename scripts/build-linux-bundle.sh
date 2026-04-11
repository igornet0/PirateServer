#!/usr/bin/env bash
# Linux x86_64 bundle — delegates to linux-bundle-build.sh amd64
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$ROOT/scripts/linux-bundle-build.sh" amd64
