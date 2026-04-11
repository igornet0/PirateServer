#!/usr/bin/env bash
# Entry point for bare-metal / VM install: points to Phase 6 bootstrap (no Docker required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
echo "PirateServer — OS-level stack bootstrap"
echo "Repository: $ROOT"
echo ""
echo "Run the interactive helper:"
echo "  bash $ROOT/scripts/bootstrap-phase6.sh"
echo ""
echo "Docker E2E: docs/DOCKER_E2E.md · make -f Makefile.docker help"
