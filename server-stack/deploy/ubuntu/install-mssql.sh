#!/usr/bin/env bash
# Install Microsoft SQL Server (supported Ubuntu versions only). Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
. /etc/os-release
UBU_VER="${VERSION_ID:-22.04}"
if [[ "$UBU_VER" != "20.04" && "$UBU_VER" != "22.04" ]]; then
  echo "MS SQL packages are only tested for Ubuntu 20.04/22.04 (this host: $UBU_VER). Exit." >&2
  exit 1
fi
curl -fsSL https://packages.microsoft.com/keys/microsoft.asc | gpg --dearmor -o /usr/share/keyrings/microsoft-prod.gpg
echo "deb [signed-by=/usr/share/keyrings/microsoft-prod.gpg] https://packages.microsoft.com/ubuntu/${UBU_VER}/mssql-server-2022 prod main" >/etc/apt/sources.list.d/mssql-server.list
apt-get update -qq
ACCEPT_EULA=Y apt-get install -y -qq mssql-server || {
  echo "mssql-server install failed — check EULA and Ubuntu compatibility." >&2
  exit 1
}
echo "mssql-server installed. Run /opt/mssql/bin/mssql-conf setup if not configured."
