#!/usr/bin/env bash
# Bump a single version source (see Makefile help: up-version).
# Usage: PROJECT=... VERSION=... ./scripts/up-version.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  echo "usage: PROJECT=<name> VERSION=<string> $0" >&2
  echo "  PROJECT=release|client|deploy_server|control_api|dashboard_ui" >&2
  echo "  VERSION must match deploy rules: [a-zA-Z0-9._-]+, len 1..128, no .. / \\" >&2
}

validate_version_string() {
  local v="$1"
  if [[ -z "$v" ]]; then
    echo "up-version: VERSION must not be empty" >&2
    return 1
  fi
  local n="${#v}"
  if (( n > 128 )); then
    echo "up-version: VERSION too long (max 128)" >&2
    return 1
  fi
  if [[ "$v" == *..* || "$v" == */* || "$v" == *\\* ]]; then
    echo "up-version: VERSION must not contain .., /, or \\" >&2
    return 1
  fi
  if [[ ! "$v" =~ ^[a-zA-Z0-9._-]+$ ]]; then
    echo "up-version: VERSION may only contain [a-zA-Z0-9._-]" >&2
    return 1
  fi
}

# Replace the first top-level `version = "..."` line ([package] is first in our crates).
bump_cargo_package_version() {
  local file="$1"
  local ver="$2"
  if [[ ! -f "$file" ]]; then
    echo "up-version: missing $file" >&2
    return 1
  fi
  UP_VERSION_NEW="$ver" perl -i -pe '
    BEGIN { $v = $ENV{UP_VERSION_NEW}; }
    if (!$done && /^version = "/) {
      s/^version = "[^"]*"/version = "$v"/;
      $done = 1;
    }
  ' "$file"
}

PROJECT="${PROJECT:-}"
VERSION="${VERSION:-}"

if [[ -z "$PROJECT" || -z "$VERSION" ]]; then
  usage
  exit 1
fi

validate_version_string "$VERSION" || exit 1

case "$PROJECT" in
  release)
    printf '%s\n' "$VERSION" >"$ROOT/VERSION"
    echo "Updated $ROOT/VERSION -> $VERSION"
    ;;
  client)
    bump_cargo_package_version "$ROOT/local-stack/client/Cargo.toml" "$VERSION"
    echo "Updated local-stack/client/Cargo.toml -> $VERSION"
    ;;
  deploy_server)
    bump_cargo_package_version "$ROOT/server-stack/server/Cargo.toml" "$VERSION"
    echo "Updated server-stack/server/Cargo.toml -> $VERSION"
    ;;
  control_api)
    bump_cargo_package_version "$ROOT/server-stack/control-api/Cargo.toml" "$VERSION"
    echo "Updated server-stack/control-api/Cargo.toml -> $VERSION"
    ;;
  dashboard_ui)
    if ! command -v npm >/dev/null 2>&1; then
      echo "up-version: npm not found (required for dashboard_ui)" >&2
      exit 1
    fi
    (
      cd "$ROOT/server-stack/frontend"
      npm version "$VERSION" --no-git-tag-version
    )
    echo "Updated server-stack/frontend package -> $VERSION"
    ;;
  *)
    echo "up-version: unknown PROJECT=$PROJECT" >&2
    usage
    exit 1
    ;;
esac
