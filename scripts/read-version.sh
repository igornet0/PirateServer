#!/usr/bin/env bash
# Print repository release version (contents of repo-root VERSION, one line, SemVer).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ ! -f "$ROOT/VERSION" ]]; then
  echo "read-version.sh: missing $ROOT/VERSION" >&2
  exit 1
fi
tr -d ' \r\n\t' <"$ROOT/VERSION"
