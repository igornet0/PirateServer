#!/usr/bin/env bash
# Install MySQL server from Ubuntu repos. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq mysql-server
systemctl enable mysql
systemctl restart mysql
echo "MySQL installed. Secure with mysql_secure_installation if exposed beyond localhost."
