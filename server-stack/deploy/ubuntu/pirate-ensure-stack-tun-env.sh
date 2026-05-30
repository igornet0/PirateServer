#!/usr/bin/env bash
# Create or upgrade /etc/pirate-stack-tun-api.env (LAN bind + REST bearer).
# Called from install.sh, pirate-apply-stack-bundle.sh, pirate-host-service.sh install stack_tun_api.
set -euo pipefail

ENV=/etc/pirate-stack-tun-api.env

gen_secret() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 32
  elif command -v python3 >/dev/null 2>&1; then
    python3 -c "import secrets; print(secrets.token_hex(32))"
  else
    echo ""
  fi
}

migrate_localhost_binds() {
  [[ -f "$ENV" ]] || return 0
  local tmp
  tmp="$(mktemp)" || return 0
  sed \
    -e 's/^STACK_TUN_HTTP_BIND=127\.0\.0\.1:9380$/STACK_TUN_HTTP_BIND=0.0.0.0:9380/' \
    -e 's/^STACK_TUN_GRPC_BIND=127\.0\.0\.1:9381$/STACK_TUN_GRPC_BIND=0.0.0.0:9381/' \
    "$ENV" >"$tmp"
  install -m 0640 -o root -g pirate "$tmp" "$ENV"
  rm -f "$tmp"
}

ensure_bearer() {
  [[ -f "$ENV" ]] || return 0
  if grep -qE '^STACK_TUN_REST_BEARER=.+' "$ENV"; then
    return 0
  fi
  local tok
  tok="$(gen_secret)"
  [[ -n "$tok" ]] || {
    echo "pirate-ensure-stack-tun-env: need openssl or python3 to generate STACK_TUN_REST_BEARER" >&2
    return 1
  }
  if grep -q '^STACK_TUN_REST_BEARER=' "$ENV"; then
    sed -i "s/^STACK_TUN_REST_BEARER=.*/STACK_TUN_REST_BEARER=${tok}/" "$ENV"
  else
    echo "STACK_TUN_REST_BEARER=${tok}" >>"$ENV"
  fi
  chmod 0640 "$ENV"
  chown root:pirate "$ENV"
  echo "pirate-ensure-stack-tun-env: set STACK_TUN_REST_BEARER in ${ENV} (use in Pirate Client stack-tun Bearer)." >&2
}

if [[ ! -f "$ENV" ]]; then
  tok="$(gen_secret)"
  if [[ -z "$tok" ]]; then
    echo "pirate-ensure-stack-tun-env: need openssl or python3 to create ${ENV}" >&2
    exit 1
  fi
  umask 077
  {
    cat <<EOF
# stack-tun-api — HTTP control (:9380) + gRPC relay (:9381). LAN-visible by default.
STACK_TUN_STATE_DIR=/var/lib/pirate/stack-tun-api
STACK_TUN_HTTP_BIND=0.0.0.0:9380
STACK_TUN_GRPC_BIND=0.0.0.0:9381
STACK_TUN_REST_BEARER=${tok}
STACK_TUN_ALLOW_UNAUTHENTICATED=0
RUST_LOG=info
EOF
  } >"$ENV"
  chmod 0640 "$ENV"
  chown root:pirate "$ENV"
  echo "pirate-ensure-stack-tun-env: created ${ENV} (HTTP/gRPC on 0.0.0.0:9380/9381). Save STACK_TUN_REST_BEARER for Pirate Client." >&2
else
  migrate_localhost_binds
  ensure_bearer
fi
