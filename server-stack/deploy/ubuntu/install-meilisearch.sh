#!/usr/bin/env bash
# Install Meilisearch — default loopback. Override PIRATE_MEILI_*, PIRATE_MEILISEARCH_VERSION, MEILI_MASTER_KEY.
# Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates

port_from_addr() {
  local a="${1:-}"
  echo "${a##*:}"
}

MEILI_VER="${PIRATE_MEILISEARCH_VERSION:-1.11.0}"
HTTP_ADDR="${PIRATE_MEILI_HTTP_ADDR:-127.0.0.1:7700}"
DB_PATH="${PIRATE_MEILI_DB_PATH:-/var/lib/pirate/meili/data}"
HTTP_PORT="$(port_from_addr "$HTTP_ADDR")"

# pirate home is often 0700; install.sh only adds o+x for UI+nginx. Service user must traverse to DB_PATH.
if [[ "$DB_PATH" == /var/lib/pirate/* ]] && [[ -d /var/lib/pirate ]]; then
  chmod o+x /var/lib/pirate
fi

ARCH="$(uname -m)"
case "$ARCH" in
x86_64) ASSET="meilisearch-linux-amd64" ;;
aarch64) ASSET="meilisearch-linux-aarch64" ;;
arm64) ASSET="meilisearch-linux-aarch64" ;;
*) echo "meilisearch: unsupported arch: $ARCH" >&2; exit 1 ;;
esac

URL="https://github.com/meilisearch/meilisearch/releases/download/v${MEILI_VER}/${ASSET}"
curl -fsSL "$URL" -o /usr/local/bin/meilisearch
chmod 0755 /usr/local/bin/meilisearch

install -d -m 0755 "$(dirname "$DB_PATH")"
install -d -m 0755 "$DB_PATH"

id -u meilisearch &>/dev/null || useradd -r -s /usr/sbin/nologin -d /var/lib/pirate/meili meilisearch
chown -R meilisearch:meilisearch /var/lib/pirate/meili

ENV_FILE="/etc/pirate-meilisearch.env"
if [[ ! -f "$ENV_FILE" ]]; then
  {
    echo "MEILI_ENV=production"
    if [[ -n "${MEILI_MASTER_KEY:-}" ]]; then
      echo "MEILI_MASTER_KEY=${MEILI_MASTER_KEY}"
    elif command -v openssl &>/dev/null; then
      echo "MEILI_MASTER_KEY=$(openssl rand -base64 32 | tr -d '/+=' | head -c 32)"
    else
      echo "MEILI_MASTER_KEY=$(tr -dc 'a-zA-Z0-9' </dev/urandom | head -c 32)"
    fi
  } >"$ENV_FILE"
  chmod 0600 "$ENV_FILE"
fi

if command -v ss &>/dev/null; then
  if ss -ltnH "sport = :$HTTP_PORT" 2>/dev/null | grep -q .; then
    echo "meilisearch: port $HTTP_PORT already in use" >&2
    exit 1
  fi
fi

chown -R meilisearch:meilisearch /var/lib/pirate/meili
chown -R meilisearch:meilisearch "$DB_PATH"

cat >/etc/systemd/system/pirate-meilisearch.service <<EOF
[Unit]
Description=Pirate Meilisearch
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=meilisearch
Group=meilisearch
Environment=HOME=/var/lib/pirate/meili
WorkingDirectory=${DB_PATH}
EnvironmentFile=-/etc/pirate-meilisearch.env
# Repair permissions on each start (root-owned LMDB files after failed run; pirate home not o+x).
ExecStartPre=-/bin/chmod o+x /var/lib/pirate
ExecStartPre=-/bin/chown -R meilisearch:meilisearch /var/lib/pirate/meili
ExecStartPre=-/bin/chown -R meilisearch:meilisearch ${DB_PATH}
ExecStart=/usr/local/bin/meilisearch --http-addr ${HTTP_ADDR} --db-path ${DB_PATH}
Restart=on-failure
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable pirate-meilisearch
systemctl restart pirate-meilisearch
echo "Meilisearch installed. HTTP: ${HTTP_ADDR} db: ${DB_PATH} — ${ENV_FILE}"
