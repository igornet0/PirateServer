#!/bin/bash
set -euo pipefail
# Ensure named volume mounts are writable by the non-root deploy user (uid 1000).
if [[ -d /data ]]; then
  chown -R deploy:deploy /data
fi
exec gosu deploy "$@"
