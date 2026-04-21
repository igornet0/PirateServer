#!/bin/bash
set -euo pipefail
if [[ -f /etc/pirate-host-agent.env ]]; then
  set -a
  # shellcheck disable=SC1091
  . /etc/pirate-host-agent.env
  set +a
fi
export RUST_LOG="${RUST_LOG:-info}"
exec /usr/local/bin/pirate-host-agent
