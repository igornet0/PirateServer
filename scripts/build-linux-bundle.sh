#!/usr/bin/env bash
# Сборка артефактов для Ubuntu x86_64 (типичный GPU-сервер) на macOS ARM.
# Требования: Node/npm, rustup target x86_64-unknown-linux-gnu, cargo-zigbuild + zig
#   (или только GNU linker для linux-gnu — см. rustup book / cross).
#
# Использование:
#   ./scripts/build-linux-bundle.sh
# Архив: dist/pirete-linux-amd64-<date>.tar.gz
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Всегда кладём артефакты в ./target репозитория (не в sandbox Cursor).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
DIST_DIR="$REPO_ROOT/dist"
STAGE="$DIST_DIR/stage/pirete-linux-amd64"
OUT_TGZ="$DIST_DIR/pirete-linux-amd64-$(date +%Y%m%d).tar.gz"

rustup target add "$TARGET_TRIPLE" >/dev/null 2>&1 || true

echo "==> frontend (npm run build)"
(
  cd "$REPO_ROOT/server-stack/frontend"
  if [[ -f package-lock.json ]]; then
    npm ci
  else
    npm install
  fi
  npm run build
)

echo "==> Rust release ($TARGET_TRIPLE)"
if command -v cargo-zigbuild >/dev/null 2>&1; then
  cargo zigbuild --release --target "$TARGET_TRIPLE" -p deploy-server -p control-api -p deploy-client
else
  echo "cargo-zigbuild not found; using cargo (нужен linker для $TARGET_TRIPLE — установите zig+cargo-zigbuild или cross)"
  cargo build --release --target "$TARGET_TRIPLE" -p deploy-server -p control-api -p deploy-client
fi

BIN_DIR="$CARGO_TARGET_DIR/$TARGET_TRIPLE/release"
for b in deploy-server control-api client; do
  if [[ ! -f "$BIN_DIR/$b" ]]; then
    echo "missing: $BIN_DIR/$b"
    exit 1
  fi
done

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/ui" "$STAGE/systemd" "$STAGE/nginx"

cp -a "$BIN_DIR/deploy-server" "$BIN_DIR/control-api" "$BIN_DIR/client" "$STAGE/bin/"
chmod +x "$STAGE/bin/"*
cp -a "$REPO_ROOT/server-stack/frontend/dist/." "$STAGE/share/ui/dist/"

cp "$REPO_ROOT/server-stack/deploy/ubuntu/deploy-server.service" "$STAGE/systemd/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/control-api.service" "$STAGE/systemd/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/nginx-pirete-site.conf" "$STAGE/nginx/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/env.example" "$STAGE/"
cp "$REPO_ROOT/server-stack/deploy/ubuntu/install.sh" "$STAGE/"
chmod +x "$STAGE/install.sh"

rm -f "$OUT_TGZ"
mkdir -p "$DIST_DIR"
( cd "$DIST_DIR/stage" && tar -czf "$OUT_TGZ" pirete-linux-amd64 )

echo ""
echo "Готово: $OUT_TGZ"
echo "На сервере: tar xzf $(basename "$OUT_TGZ") && cd pirete-linux-amd64 && sudo ./install.sh"
