#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop redis-server 2>/dev/null || true
systemctl disable redis-server 2>/dev/null || true
apt-get remove -y --purge redis-server || true
apt-get autoremove -y || true
echo "Redis removed."
