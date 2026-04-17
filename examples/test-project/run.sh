#!/bin/sh
set -e

cd "$(dirname "$0")"
export NODE_ENV='production'

# Match deploy: artifacts are under `.release/` (see pirate.toml [build].output_path).
cd .release
exec npm start
