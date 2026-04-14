#!/usr/bin/env bash
# Invoked inside rust:bookworm (see linux-bundle-build-rust-in-docker.sh). Installs native deps for
# deploy-client (xcap → libxcb, dbus) and runs cargo release build for the Linux target triple.
set -euo pipefail
: "${TARGET_TRIPLE:?}"
: "${CARGO_TARGET_DIR:?}"

apt-get update -qq
apt-get install -y --no-install-recommends \
  pkg-config \
  ca-certificates \
  build-essential \
  libssl-dev \
  libdbus-1-dev \
  libxcb1-dev \
  libxrandr-dev \
  libxfixes-dev \
  libxinerama-dev

cd /work
export CARGO_TARGET_DIR
rustup target add "$TARGET_TRIPLE" 2>/dev/null || true
exec cargo build --release --target "$TARGET_TRIPLE" \
  -p deploy-server -p control-api -p deploy-client
