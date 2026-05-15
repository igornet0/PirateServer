#!/usr/bin/env bash
# Fail if repo-root VERSION (used by read-version.sh / desktop dist) diverges from
# deploy-client (pirate CLI) package version in local-stack/client/Cargo.toml.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if [[ ! -f VERSION ]]; then
  echo "error: missing $REPO_ROOT/VERSION" >&2
  exit 1
fi

file_ver="$(tr -d ' \r\n\t' <VERSION)"
toml_ver="$(grep -E '^version = ' "$REPO_ROOT/local-stack/client/Cargo.toml" | head -n 1 | sed -E 's/^version = "([^"]+)".*/\1/')"

if [[ "$file_ver" != "$toml_ver" ]]; then
  echo "error: VERSION file (${file_ver}) != deploy-client in local-stack/client/Cargo.toml (${toml_ver})" >&2
  echo "  Bump both together before a release (see client README: CLI version vs PATH)." >&2
  exit 1
fi

echo "OK: VERSION and deploy-client/Cargo.toml agree (${file_ver})"
