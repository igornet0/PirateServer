#!/usr/bin/env bash
# Node.js + npm from Ubuntu repositories (predictable; no curl|bash). Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq nodejs npm
echo "Node: $(node -v 2>/dev/null || echo '?')"
echo "npm: $(npm -v 2>/dev/null || echo '?')"
