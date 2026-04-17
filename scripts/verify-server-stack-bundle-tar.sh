#!/usr/bin/env bash
# Verify a Linux server-stack .tar.gz unpacks to a layout deploy-server expects for OTA
# (bin/deploy-server + bin/control-api under pirate-linux-* or flat top-level).
# Also requires lib/pirate/99-pirate-smb.sudoers.fragment with pirate-host-service.sh NOPASSWD
# so control-api host-services install does not fail with "sudo: a password is required".
#
# Usage: ./scripts/verify-server-stack-bundle-tar.sh <path/to/pirate-linux-*.tar.gz>
# Exit 0 if OK, 1 otherwise.
set -euo pipefail

TGZ="${1:?usage: $0 <pirate-linux-*.tar.gz>}"
if [[ ! -f "$TGZ" ]]; then
  echo "error: not a file: $TGZ" >&2
  exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

tar xzf "$TGZ" -C "$TMP"

has_bins() {
  local d="$1"
  [[ -f "$d/bin/deploy-server" && -f "$d/bin/control-api" ]]
}

matches=()
for d in "$TMP"/*; do
  [[ -d "$d" ]] || continue
  if has_bins "$d"; then
    matches+=("$d")
  fi
done

BUNDLE_ROOT=""
if [[ ${#matches[@]} -eq 1 ]]; then
  BUNDLE_ROOT="${matches[0]}"
elif [[ ${#matches[@]} -eq 0 ]] && has_bins "$TMP"; then
  BUNDLE_ROOT="$TMP"
else
  echo "error: expected exactly one directory with bin/deploy-server and bin/control-api (or flat layout at archive root)" >&2
  echo "extract top-level:" >&2
  ls -la "$TMP" >&2 || true
  exit 1
fi

echo "OK: bundle root $BUNDLE_ROOT"

FRAG="$BUNDLE_ROOT/lib/pirate/99-pirate-smb.sudoers.fragment"
if [[ ! -f "$FRAG" ]]; then
  echo "error: missing $FRAG (include 99-pirate-smb.sudoers.fragment in lib/pirate for OTA sudoers)" >&2
  exit 1
fi
if ! grep -q 'pirate-host-service\.sh' "$FRAG"; then
  echo "error: $FRAG must NOPASSWD-list pirate-host-service.sh (host-services install from control-api)" >&2
  exit 1
fi

echo "OK: sudoers fragment lists pirate-host-service.sh"

RM_NODE="$BUNDLE_ROOT/lib/pirate/remove-nodejs-runtime.sh"
if [[ ! -f "$RM_NODE" ]]; then
  echo "error: missing $RM_NODE (linux-bundle-build must ship remove-*.sh for pirate-host-service.sh remove)" >&2
  exit 1
fi

echo "OK: remove-nodejs-runtime.sh present (host-service remove path)"
