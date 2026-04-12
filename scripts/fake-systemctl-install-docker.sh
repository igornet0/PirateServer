#!/usr/bin/env bash
# Minimal systemctl stub for install.sh inside Docker (no real systemd PID 1).
# Handles only what server-stack/deploy/ubuntu/install.sh invokes.
set -euo pipefail

wait_tcp() {
  local host=$1
  local port=$2
  local n=0
  while [[ "$n" -lt 60 ]]; do
    if bash -c "exec 3<>/dev/tcp/${host}/${port}" 2>/dev/null; then
      exec 3<&- 2>/dev/null || true
      return 0
    fi
    n=$((n + 1))
    sleep 0.2
  done
  return 1
}

cmd="${1:-}"
case "$cmd" in
daemon-reload) exit 0 ;;
enable) exit 0 ;;
restart)
  unit="${2:-}"
  case "$unit" in
  deploy-server.service)
    pkill -x deploy-server 2>/dev/null || true
    sleep 0.5
    runuser -u pirate -- /bin/bash -c '
      set -a
      . /etc/pirate-deploy.env
      set +a
      exec /usr/local/bin/deploy-server --root /var/lib/pirate/deploy -p 50051 --bind 0.0.0.0
    ' &
    wait_tcp 127.0.0.1 50051 || {
      echo "fake-systemctl: timeout waiting for gRPC 50051" >&2
      exit 1
    }
    exit 0
    ;;
  control-api.service)
    pkill -x control-api 2>/dev/null || true
    sleep 0.5
    runuser -u pirate -- /bin/bash -c '
      set -a
      . /etc/pirate-deploy.env
      set +a
      exec /usr/local/bin/control-api --deploy-root /var/lib/pirate/deploy
    ' &
    api_port=8080
    if [[ -f /etc/pirate-deploy.env ]]; then
      api_port="$(grep -E '^CONTROL_API_PORT=' /etc/pirate-deploy.env 2>/dev/null | tail -1 | cut -d= -f2- | tr -d '\r' || true)"
      [[ -z "${api_port:-}" ]] && api_port=8080
    fi
    wait_tcp 127.0.0.1 "$api_port" || {
      echo "fake-systemctl: timeout waiting for control-api on $api_port" >&2
      exit 1
    }
    exit 0
    ;;
  nginx.service)
    pkill -x nginx 2>/dev/null || true
    exit 0
    ;;
  *)
    echo "fake-systemctl: unsupported unit: $*" >&2
    exit 1
    ;;
  esac
  ;;
*)
  echo "fake-systemctl: unsupported: $*" >&2
  exit 1
  ;;
esac
