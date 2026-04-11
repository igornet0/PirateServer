#!/usr/bin/env bash
# Write dist/release-manifest.json after workspace release + dashboard build.
# Usage: from repo root; expects VERSION, optional git, package.json files.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REL="$("$ROOT/scripts/read-version.sh")"
mkdir -p "$ROOT/dist"

DS_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/server-stack/server/Cargo.toml")"
CA_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/server-stack/control-api/Cargo.toml")"
CLI_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/local-stack/client/Cargo.toml")"
DESK_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/local-stack/desktop-client/Cargo.toml")"
TAURI_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/local-stack/desktop-ui/src-tauri/Cargo.toml")"

DASH_NPM="0.0.0"
if [[ -f "$ROOT/server-stack/frontend/package.json" ]]; then
  DASH_NPM="$(node -p "require(process.argv[1]).version" "$ROOT/server-stack/frontend/package.json" 2>/dev/null || echo "0.0.0")"
fi
PIRATE_UI_NPM="0.0.0"
if [[ -f "$ROOT/local-stack/desktop-ui/package.json" ]]; then
  PIRATE_UI_NPM="$(node -p "require(process.argv[1]).version" "$ROOT/local-stack/desktop-ui/package.json" 2>/dev/null || echo "0.0.0")"
fi

GIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
GIT_DESCRIBE="$(git -C "$ROOT" describe --always --dirty 2>/dev/null || echo "$GIT")"
BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

export REL DS_VER CA_VER CLI_VER DESK_VER TAURI_VER DASH_NPM PIRATE_UI_NPM GIT GIT_DESCRIBE BUILT_AT ROOT
python3 <<'PY'
import json, os
root = os.environ["ROOT"]
out = {
    "release": os.environ["REL"],
    "built_at": os.environ["BUILT_AT"],
    "git": os.environ["GIT"],
    "git_describe": os.environ["GIT_DESCRIBE"],
    "crates": {
        "deploy_server": os.environ["DS_VER"],
        "control_api": os.environ["CA_VER"],
        "deploy_client": os.environ["CLI_VER"],
        "pirate_desktop": os.environ["DESK_VER"],
        "pirate_client_tauri": os.environ["TAURI_VER"],
    },
    "npm": {
        "server_stack_frontend": os.environ["DASH_NPM"],
        "local_stack_desktop_ui": os.environ["PIRATE_UI_NPM"],
    },
    "artifacts": {
        "rust_release": "target/release/ (native triple)",
        "dashboard_static": "server-stack/frontend/dist/",
    },
}
path = os.path.join(root, "dist", "release-manifest.json")
with open(path, "w", encoding="utf-8") as f:
    json.dump(out, f, indent=2)
    f.write("\n")
print("Wrote", path)
PY
