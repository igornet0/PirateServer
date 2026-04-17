#!/usr/bin/env bash
# Stop and remove nginx packages. May break dashboard reverse proxy. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop nginx 2>/dev/null || true
systemctl disable nginx 2>/dev/null || true
apt-get remove -y --purge nginx nginx-common || true
apt-get autoremove -y || true
echo "nginx removed (if installed)."
