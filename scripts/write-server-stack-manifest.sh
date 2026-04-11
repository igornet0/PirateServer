#!/usr/bin/env bash
# Write server-stack-manifest.json into a staged Linux bundle directory.
# Usage: write-server-stack-manifest.sh <stage_dir> <rust_target_triple> <repo_root> [ui_build]
# ui_build: 1 (default) = dashboard static included in bundle; 0 = .bundle-no-ui (no dashboard in archive).
set -euo pipefail

STAGE="${1:?stage dir}"
TARGET_TRIPLE="${2:?target triple}"
REPO_ROOT="${3:?repo root}"
UI_BUILD="${4:-1}"
case "${UI_BUILD,,}" in
  0|false|no|off) UI_BUILD=0 ;;
  *) UI_BUILD=1 ;;
esac

ROOT="$REPO_ROOT"
REL="$("$ROOT/scripts/read-version.sh")"

DS_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/server-stack/server/Cargo.toml")"
CA_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/server-stack/control-api/Cargo.toml")"
CLI_VER="$(awk -F '"' '/^version/ {print $2; exit}' "$ROOT/local-stack/client/Cargo.toml")"

DASH_UI=""
if [[ "$UI_BUILD" == "1" ]]; then
  if [[ -f "$ROOT/server-stack/frontend/package.json" ]]; then
    DASH_UI="$(node -p "require(process.argv[1]).version" "$ROOT/server-stack/frontend/package.json" 2>/dev/null || awk -F '"' '/"version"/{print $4; exit}' "$ROOT/server-stack/frontend/package.json")"
  fi
  if [[ -z "${DASH_UI:-}" ]]; then
    DASH_UI="0.0.0"
  fi
fi

GIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
GIT_DESCRIBE="$(git -C "$ROOT" describe --always --dirty 2>/dev/null || echo "$GIT")"
BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

export REL STAGE TARGET_TRIPLE DS_VER CA_VER CLI_VER DASH_UI UI_BUILD GIT GIT_DESCRIBE BUILT_AT
python3 <<'PY'
import json, os
stage = os.environ["STAGE"]
ui_build = os.environ["UI_BUILD"] == "1"
out = {
    "release": os.environ["REL"],
    "target": os.environ["TARGET_TRIPLE"],
    "deploy_server": os.environ["DS_VER"],
    "control_api": os.environ["CA_VER"],
    "client": os.environ["CLI_VER"],
    "dashboard_ui_bundled": ui_build,
    "git": os.environ["GIT"],
    "git_describe": os.environ["GIT_DESCRIBE"],
    "built_at": os.environ["BUILT_AT"],
}
if ui_build:
    out["dashboard_ui"] = os.environ["DASH_UI"]
else:
    out["dashboard_ui"] = None
path = os.path.join(stage, "server-stack-manifest.json")
with open(path, "w", encoding="utf-8") as f:
    json.dump(out, f, indent=2)
    f.write("\n")
PY
