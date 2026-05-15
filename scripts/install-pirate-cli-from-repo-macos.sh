#!/usr/bin/env bash
# Build deploy-client `pirate` from this repo and install to /usr/local/bin (sudo).
# Use when `pirate --version` still shows an old client= after git pull — PATH points at an old binary.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
echo "==> cargo build -p deploy-client --bin pirate --release"
cargo build -p deploy-client --bin pirate --release
BIN="$ROOT/target/release/pirate"
if [[ ! -f "$BIN" ]]; then
  echo "error: missing $BIN" >&2
  exit 1
fi
echo "==> built:"
"$BIN" --version
echo "==> sudo install -> /usr/local/bin/pirate (enter Mac password in this terminal)"
sudo install -m 0755 "$BIN" /usr/local/bin/pirate
echo "==> installed:"
/usr/local/bin/pirate --version
