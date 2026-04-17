#!/usr/bin/env bash
# Removes PostgreSQL server packages; local cluster data may be deleted. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
systemctl stop postgresql 2>/dev/null || true
systemctl disable postgresql 2>/dev/null || true
apt-get remove -y --purge 'postgresql*' || true
apt-get autoremove -y || true
echo "PostgreSQL server packages removed."
