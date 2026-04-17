#!/usr/bin/env bash
# Python 3 + venv + pip from Ubuntu repos. Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq python3 python3-venv python3-pip
python3 --version
