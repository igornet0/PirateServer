#!/usr/bin/env bash
# Smoke checks for PaaS CLI paths (no server required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== cargo test deploy-core =="
cargo test -q -p deploy-core

echo "== pirate init-project (temp dir) =="
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/fake-node"
echo '{"name":"x","scripts":{"start":"node index.js","build":"echo build","test":"echo test"}}' > "$TMP/fake-node/package.json"
cargo run -q -p deploy-client --bin pirate -- init-project "$TMP/fake-node"
test -f "$TMP/fake-node/pirate.toml"

echo "== pirate scan-project =="
cargo run -q -p deploy-client --bin pirate -- scan-project "$TMP/fake-node"

echo "paas-smoke OK"
