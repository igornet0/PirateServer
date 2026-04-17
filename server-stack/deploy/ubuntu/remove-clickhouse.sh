#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop clickhouse-server 2>/dev/null || true
systemctl disable clickhouse-server 2>/dev/null || true
apt-get remove -y --purge clickhouse-server clickhouse-client clickhouse-common-static || true
rm -f /etc/apt/sources.list.d/clickhouse.list 2>/dev/null || true
apt-get autoremove -y || true
echo "ClickHouse packages removed."
