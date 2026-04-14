#!/bin/bash
set -euo pipefail
if [[ -f /etc/pirate-deploy.env ]]; then
  set -a
  # shellcheck disable=SC1091
  . /etc/pirate-deploy.env
  set +a
fi
export RUST_LOG="${RUST_LOG:-info}"
exec /usr/local/bin/deploy-server --root /var/lib/pirate/deploy -p 50051
