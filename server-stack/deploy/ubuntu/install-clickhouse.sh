#!/usr/bin/env bash
# Install ClickHouse from official packages. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq apt-transport-https ca-certificates curl gnupg
curl -fsSL https://packages.clickhouse.com/clickhouse-keyring.gpg | gpg --dearmor -o /usr/share/keyrings/clickhouse-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/clickhouse-keyring.gpg] https://packages.clickhouse.com/deb stable main" >/etc/apt/sources.list.d/clickhouse.list
apt-get update -qq
apt-get install -y -qq clickhouse-server clickhouse-client
systemctl enable clickhouse-server
systemctl restart clickhouse-server
echo "ClickHouse installed (HTTP default 8123)."
