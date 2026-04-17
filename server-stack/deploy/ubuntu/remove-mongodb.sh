#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop mongod 2>/dev/null || true
systemctl disable mongod 2>/dev/null || true
apt-get remove -y --purge mongodb-org mongodb-org-* mongodb-mongosh || true
apt-get autoremove -y || true
echo "MongoDB packages removed."
