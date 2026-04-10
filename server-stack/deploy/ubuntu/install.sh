#!/usr/bin/env bash
# Установка на Ubuntu (x86_64 или ARM64 при наличии нативных бинарников в комплекте).
# Запуск: sudo ./install.sh
# Каталог: распакованный pirete-linux-amd64/ (рядом с bin/, share/, install.sh).

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

BIN_LOCAL="$SCRIPT_DIR/bin"
UI_SRC="$SCRIPT_DIR/share/ui/dist"
SYSTEMD_SRC="$SCRIPT_DIR/systemd"
NGINX_SRC="$SCRIPT_DIR/nginx"

for p in "$BIN_LOCAL/deploy-server" "$BIN_LOCAL/control-api" "$BIN_LOCAL/client" "$UI_SRC/index.html"; do
  if [[ ! -e "$p" ]]; then
    echo "Не найдено: $p — запускайте из распакованного архива pirete-linux-amd64." >&2
    exit 1
  fi
done

HOST_ARCH="$(uname -m)"
BIN_ARCH="$(file -b "$BIN_LOCAL/deploy-server" 2>/dev/null || true)"
if [[ "$HOST_ARCH" == "aarch64" ]] && [[ "$BIN_ARCH" == *"x86-64"* ]]; then
  echo "Ошибка: бинарники x86_64, а сервер ARM64. Соберите архив с TARGET_TRIPLE=aarch64-unknown-linux-gnu" >&2
  exit 1
fi

echo "==> apt: nginx, postgresql, openssl"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq nginx postgresql openssl ca-certificates

echo "==> пользователь и каталоги"
if ! id deploy &>/dev/null; then
  useradd --system --create-home --home-dir /var/lib/pirete --shell /usr/sbin/nologin deploy
fi
install -d -o deploy -g deploy -m 0755 /var/lib/pirete/deploy
install -d -o deploy -g deploy -m 0755 /var/lib/pirete/ui

echo "==> бинарники -> /usr/local/bin"
install -m 0755 "$BIN_LOCAL/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_LOCAL/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_LOCAL/client" /usr/local/bin/client

echo "==> frontend -> /var/lib/pirete/ui/dist"
rm -rf /var/lib/pirete/ui/dist
cp -a "$UI_SRC" /var/lib/pirete/ui/dist
chown -R deploy:deploy /var/lib/pirete/ui

echo "==> PostgreSQL: пользователь и БД deploy"
PG_PASS="${PIRETE_DB_PASSWORD:-$(openssl rand -base64 24 | tr -d '/+=' | head -c 32)}"
if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='deploy'" | grep -q 1; then
  sudo -u postgres psql -c "ALTER USER deploy WITH PASSWORD '$PG_PASS';" >/dev/null
else
  sudo -u postgres psql -c "CREATE USER deploy WITH PASSWORD '$PG_PASS';" >/dev/null
fi
if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='deploy'" | grep -q 1; then
  :
else
  sudo -u postgres psql -c "CREATE DATABASE deploy OWNER deploy;" >/dev/null
fi

# Доступ по TCP с 127.0.0.1 (DATABASE_URL с 127.0.0.1)
PG_VER="$(ls /etc/postgresql 2>/dev/null | sort -V | tail -1 || true)"
PG_HBA=""
if [[ -n "$PG_VER" ]]; then
  PG_HBA="/etc/postgresql/${PG_VER}/main/pg_hba.conf"
fi
if [[ -n "$PG_HBA" && -f "$PG_HBA" ]]; then
  if ! grep -qF 'host deploy deploy 127.0.0.1/32 scram-sha-256' "$PG_HBA"; then
    echo 'host deploy deploy 127.0.0.1/32 scram-sha-256' >> "$PG_HBA"
    systemctl reload postgresql
  fi
fi

echo "==> /etc/pirete-deploy.env"
DATABASE_URL="postgresql://deploy:${PG_PASS}@127.0.0.1:5432/deploy"
umask 077
cat >/etc/pirete-deploy.env <<EOF
DATABASE_URL=${DATABASE_URL}
DEPLOY_ROOT=/var/lib/pirete/deploy
GRPC_ENDPOINT=http://[::1]:50051
CONTROL_API_PORT=8080
RUST_LOG=info
EOF
chmod 0640 /etc/pirete-deploy.env
chown root:deploy /etc/pirete-deploy.env

echo "==> systemd"
install -m 0644 "$SYSTEMD_SRC/deploy-server.service" /etc/systemd/system/deploy-server.service
install -m 0644 "$SYSTEMD_SRC/control-api.service" /etc/systemd/system/control-api.service
systemctl daemon-reload
systemctl enable deploy-server.service control-api.service

echo "==> nginx"
install -m 0644 "$NGINX_SRC/nginx-pirete-site.conf" /etc/nginx/sites-available/pirete
if [[ -f /etc/nginx/sites-enabled/default ]]; then
  rm -f /etc/nginx/sites-enabled/default
fi
ln -sf /etc/nginx/sites-available/pirete /etc/nginx/sites-enabled/pirete
nginx -t
systemctl enable nginx
systemctl restart nginx

echo "==> запуск сервисов"
systemctl restart postgresql
systemctl restart deploy-server.service
sleep 1
systemctl restart control-api.service

echo ""
echo "Готово."
echo "  Пароль БД (также в /etc/pirete-deploy.env): $PG_PASS"
echo "  UI:        http://$(hostname -I 2>/dev/null | awk '{print $1}')/"
echo "  API health: curl -s http://127.0.0.1:8080/health"
echo "  Клиент:    client status   (с сервера; endpoint по умолчанию [::1]:50051)"
echo ""
echo "Логи: journalctl -u deploy-server -f   /   journalctl -u control-api -f"
