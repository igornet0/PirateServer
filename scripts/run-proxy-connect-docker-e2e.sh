#!/usr/bin/env bash
# Back-compat wrapper: same stack bootstrap as full suite, PROXY_TEST_SUITE=basic.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PROXY_TEST_SUITE="${PROXY_TEST_SUITE:-basic}"
exec bash "$ROOT/scripts/run-proxy-test-suite.sh"
