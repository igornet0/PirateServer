#!/usr/bin/env bash
set -euo pipefail
DIR="${STACK_TUN_STATE_DIR:-/var/lib/pirate/stack-tun-api}"
mkdir -p "$DIR"
PROFILES="$DIR/profiles.json"
if [[ ! -f "$PROFILES" ]]; then
  cat >"$PROFILES" <<'JSON'
{
  "version": 1,
  "profiles": [
    {
      "id": "e2e-bus-listen",
      "name": "e2e-listen",
      "role": "listen",
      "mode": "requestBus",
      "linkKind": "local",
      "routeTags": [],
      "allowedHosts": [],
      "allowedPaths": [],
      "connectorAllowPubkeyB64": [],
      "enabled": true,
      "targetHost": "",
      "targetPort": 0,
      "maxPendingStreams": 128,
      "streamOfferTtlSecs": 300,
      "pullWaitMs": 30000
    }
  ],
  "routes": []
}
JSON
  echo "[listener] seeded $PROFILES"
fi
exec stack-tun-api
