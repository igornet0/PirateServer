#!/usr/bin/env bash
# Unmount SMB shares before removing cifs-utils if needed. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get remove -y --purge cifs-utils || true
apt-get autoremove -y || true
echo "cifs-utils removed."
