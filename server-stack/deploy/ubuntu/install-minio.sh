#!/usr/bin/env bash
# Install MinIO (S3) — defaults loopback. Override via PIRATE_MINIO_* / MINIO_ROOT_* (see control-api host-services install).
# Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates

port_from_addr() {
  local a="${1:-}"
  echo "${a##*:}"
}

API_ADDR="${PIRATE_MINIO_API_ADDR:-127.0.0.1:9000}"
CONSOLE_ADDR="${PIRATE_MINIO_CONSOLE_ADDR:-127.0.0.1:9001}"
DATA_DIR="${PIRATE_MINIO_DATA_DIR:-/var/lib/pirate/minio}"
API_PORT="$(port_from_addr "$API_ADDR")"
CONSOLE_PORT="$(port_from_addr "$CONSOLE_ADDR")"

# pirate home is often 0700; install.sh only adds o+x for UI+nginx. MinIO user must traverse to DATA_DIR.
if [[ "$DATA_DIR" == /var/lib/pirate/* ]] && [[ -d /var/lib/pirate ]]; then
  chmod o+x /var/lib/pirate
fi

ARCH="$(uname -m)"
case "$ARCH" in
x86_64) MINIO_ARCH=amd64 ;;
aarch64|arm64) MINIO_ARCH=arm64 ;;
*) echo "minio: unsupported arch: $ARCH" >&2; exit 1 ;;
esac

MINIO_URL="https://dl.min.io/server/minio/release/linux-${MINIO_ARCH}/minio"
curl -fsSL "$MINIO_URL" -o /usr/local/bin/minio
chmod 0755 /usr/local/bin/minio

install -d -m 0755 "$DATA_DIR"

id -u minio &>/dev/null || useradd -r -s /usr/sbin/nologin -d "$DATA_DIR" minio
chown -R minio:minio "$DATA_DIR"

ENV_FILE="/etc/pirate-minio.env"
if [[ ! -f "$ENV_FILE" ]]; then
  {
    if [[ -n "${MINIO_ROOT_USER:-}" ]]; then
      echo "MINIO_ROOT_USER=${MINIO_ROOT_USER}"
    else
      echo "MINIO_ROOT_USER=minioadmin"
    fi
    if [[ -n "${MINIO_ROOT_PASSWORD:-}" ]]; then
      echo "MINIO_ROOT_PASSWORD=${MINIO_ROOT_PASSWORD}"
    elif command -v openssl &>/dev/null; then
      echo "MINIO_ROOT_PASSWORD=$(openssl rand -base64 24 | tr -dc 'a-zA-Z0-9' | head -c 24)"
    else
      echo "MINIO_ROOT_PASSWORD=$(tr -dc 'a-zA-Z0-9' </dev/urandom | head -c 24)"
    fi
  } >"$ENV_FILE"
  chmod 0600 "$ENV_FILE"
fi

check_port() {
  local p="$1"
  if command -v ss &>/dev/null; then
    if ss -ltnH "sport = :$p" 2>/dev/null | grep -q .; then
      echo "minio: port $p already in use" >&2
      exit 1
    fi
  elif command -v fuser &>/dev/null; then
    fuser "$p/tcp" &>/dev/null && { echo "minio: port $p in use" >&2; exit 1; } || true
  fi
}
check_port "$API_PORT"
check_port "$CONSOLE_PORT"

cat >/etc/systemd/system/pirate-minio.service <<EOF
[Unit]
Description=Pirate MinIO (S3-compatible storage)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=minio
Group=minio
EnvironmentFile=-/etc/pirate-minio.env
ExecStart=/usr/local/bin/minio server ${DATA_DIR} --address ${API_ADDR} --console-address ${CONSOLE_ADDR}
Restart=on-failure
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable pirate-minio
systemctl restart pirate-minio
echo "MinIO installed. API: ${API_ADDR} console: ${CONSOLE_ADDR} — $ENV_FILE"
