#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop mssql-server 2>/dev/null || true
systemctl disable mssql-server 2>/dev/null || true
apt-get remove -y --purge mssql-server || true
rm -f /etc/apt/sources.list.d/mssql-server.list 2>/dev/null || true
apt-get autoremove -y || true
echo "MS SQL Server package removed."
