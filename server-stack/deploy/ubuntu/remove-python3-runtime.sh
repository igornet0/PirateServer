#!/usr/bin/env bash
# Remove optional Python tooling only; keeps system python3. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get remove -y --purge python3-pip python3-venv || true
apt-get autoremove -y || true
echo "python3-pip / python3-venv removed if present; python3 may remain."
