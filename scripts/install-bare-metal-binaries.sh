#!/usr/bin/env bash
# Build release binaries and install to /usr/local/bin (requires write access).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build --release -p deploy-server -p control-api -p tunnel-gateway
install -m 0755 target/release/deploy-server /usr/local/bin/
install -m 0755 target/release/control-api /usr/local/bin/
install -m 0755 target/release/tunnel-gateway /usr/local/bin/
echo "Installed deploy-server, control-api, tunnel-gateway to /usr/local/bin"
