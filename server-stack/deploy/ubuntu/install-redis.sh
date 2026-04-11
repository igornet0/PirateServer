#!/usr/bin/env bash
# Install Redis server (localhost by default). Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq redis-server
systemctl enable redis-server
systemctl restart redis-server
echo "Redis installed."
