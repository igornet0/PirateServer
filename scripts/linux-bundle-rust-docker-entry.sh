#!/usr/bin/env bash
# Invoked inside rust:bookworm (see linux-bundle-build-rust-in-docker.sh). Installs native deps for
# deploy-client (xcap → libxcb, dbus) and runs cargo release build for the Linux target triple.
set -euo pipefail
: "${TARGET_TRIPLE:?}"
: "${CARGO_TARGET_DIR:?}"

# Fail fast if the Rust toolchain in the image is broken (e.g. librustc_driver: "no loadable segments"; Apple Silicon + amd64 emulation, etc.)
if ! rustc -vV 2>/dev/null; then
  if [[ "${HOST_MACHINE:-}" == "arm64" && "$TARGET_TRIPLE" == "x86_64-unknown-linux-gnu" ]]; then
    echo "linux-bundle-rust-docker-entry: rustc failed (Apple Silicon + linux/amd64). See: scripts/preflight-linux-rust-docker.sh  (Colima: colima start --arch x86_64, docker context use colima)" >&2
  else
    echo "linux-bundle-rust-docker-entry: rustc in the container failed (broken image, libc mismatch, or bad emulation)." >&2
  fi
  echo "  Try:  DOCKER_RUST_IMAGE_PULL=1  or  DOCKER_RUST_IMAGE=rust:1.88-bookworm  (Linux: LINUX_BUNDLE_HOST_BUILD=1 if host has deps for xcb/dbus links)" >&2
  exit 127
fi

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
if [[ "${PIRATE_DOCKER_CROSS_FALLBACK:-0}" == "1" ]]; then
  # Fallback mode: run container on host architecture and install cross sysroot/toolchain
  # for the target triple, so build works even when target platform emulation is broken.
  case "$TARGET_TRIPLE" in
    aarch64-unknown-linux-gnu)
      dpkg --add-architecture arm64
      apt-get update -qq
      apt-get install -y --no-install-recommends \
        gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross \
        libssl-dev:arm64 libdbus-1-dev:arm64 libxcb1-dev:arm64 \
        libxrandr-dev:arm64 libxfixes-dev:arm64 libxinerama-dev:arm64
      export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
      export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
      export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
      export PKG_CONFIG_ALLOW_CROSS=1
      export PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig
      ;;
    x86_64-unknown-linux-gnu)
      dpkg --add-architecture amd64
      apt-get update -qq
      apt-get install -y --no-install-recommends \
        gcc-x86-64-linux-gnu g++-x86-64-linux-gnu libc6-dev-amd64-cross \
        libssl-dev:amd64 libdbus-1-dev:amd64 libxcb1-dev:amd64 \
        libxrandr-dev:amd64 libxfixes-dev:amd64 libxinerama-dev:amd64
      export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
      export CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
      export CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++
      export PKG_CONFIG_ALLOW_CROSS=1
      export PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig
      ;;
  esac
fi
exec cargo build --release --target "$TARGET_TRIPLE" \
  -p deploy-server -p control-api -p deploy-client -p pirate-host-agent -p stack-tun-api
