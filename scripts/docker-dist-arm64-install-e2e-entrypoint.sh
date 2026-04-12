#!/usr/bin/env bash
# Container entry: extract arm64 dist tarball, run install.sh (with fake systemctl), then client auth + version checks.
set -euo pipefail
export PATH="/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin"

TGZ="$(ls -t /dist/pirate-linux-aarch64-*.tar.gz 2>/dev/null | grep -vF -- '-no-ui-' | head -1 || true)"
if [[ -z "$TGZ" ]]; then
  TGZ="$(ls -t /dist/pirate-linux-aarch64-*.tar.gz 2>/dev/null | head -1 || true)"
fi
if [[ -z "$TGZ" || ! -f "$TGZ" ]]; then
  echo "error: no /dist/pirate-linux-aarch64-*.tar.gz (mount repo dist/ and run make dist-arm64-linux)" >&2
  exit 1
fi
echo "==> using bundle: $TGZ"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -xzf "$TGZ" -C "$work"
shopt -s nullglob
stages=("$work"/pirate-linux-aarch64*)
if [[ ${#stages[@]} -ne 1 ]]; then
  echo "error: expected exactly one pirate-linux-aarch64 dir in tarball" >&2
  exit 1
fi
cd "${stages[0]}"

echo "==> install.sh (non-interactive, backend only)"
export pirate_NONINTERACTIVE=1
./install.sh

echo "==> wait gRPC after install"
for _ in $(seq 1 60); do
  if bash -c "exec 3<>/dev/tcp/127.0.0.1/50051" 2>/dev/null; then
    exec 3<&- 2>/dev/null || true
    break
  fi
  sleep 0.2
done

BUNDLE_FILE=$(mktemp)
trap 'rm -rf "$work"; rm -f "$BUNDLE_FILE"' EXIT
runuser -u pirate -- /bin/bash -c '
  set -a
  . /etc/pirate-deploy.env
  set +a
  exec /usr/local/bin/deploy-server --root /var/lib/pirate/deploy print-install-bundle
' >"$BUNDLE_FILE"

CFG=$(mktemp -d)
trap 'rm -rf "$work" "$CFG"; rm -f "$BUNDLE_FILE"' EXIT
export XDG_CONFIG_HOME="$CFG"

echo "==> client auth (pair + GetStatus)"
/usr/local/bin/client auth "$BUNDLE_FILE"

echo "==> client status (signed)"
/usr/local/bin/client status

echo "==> client --version"
/usr/local/bin/client --version

grpc_url="$(grep -E '^DEPLOY_GRPC_PUBLIC_URL=' /etc/pirate-deploy.env | tail -1 | cut -d= -f2- | tr -d '\r')"
[[ -n "$grpc_url" ]]

echo "==> client --version-all (with gRPC endpoint)"
/usr/local/bin/client --endpoint "$grpc_url" --version-all

echo "OK: dist arm64 + install.sh + client auth + version checks"
