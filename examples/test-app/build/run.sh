#!/bin/sh
# Test app for local E2E: stays in foreground so deploy-server keeps "running".
cd "$(dirname "$0")" || exit 1
exec sleep 86400
