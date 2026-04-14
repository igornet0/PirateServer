#!/usr/bin/env bash
# Deprecated: logic lives in scripts/proxy-test/ and scripts/run-proxy-test-suite.sh
set -euo pipefail
echo "proxy-connect-docker-e2e.sh is deprecated." >&2
echo "From repo root run: ./scripts/run-proxy-test-suite.sh  (PROXY_TEST_SUITE=basic by default)" >&2
echo "Or: make -f Makefile.docker proxy-test-basic" >&2
exit 1
