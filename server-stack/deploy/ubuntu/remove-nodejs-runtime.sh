#!/usr/bin/env bash
# Remove Node.js and npm packages from Ubuntu repos. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get remove -y --purge nodejs npm || true
apt-get autoremove -y || true
echo "Node.js / npm packages removed (if installed)."
