#!/usr/bin/env bash
# Removes MySQL server; databases on this host may be lost. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop mysql 2>/dev/null || true
systemctl disable mysql 2>/dev/null || true
apt-get remove -y --purge mysql-server mysql-client mysql-common || true
apt-get autoremove -y || true
echo "MySQL server packages removed."
