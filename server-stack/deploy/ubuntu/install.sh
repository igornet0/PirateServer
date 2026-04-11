#!/usr/bin/env bash
# Установка на Ubuntu (x86_64 или ARM64 при наличии нативных бинарников в комплекте).
# Запуск: sudo ./install.sh
# Во время установки будет запрос домена (можно Enter — тогда UI по http://IP:80/).
# Без вопроса (автоматизация): sudo pirate_NONINTERACTIVE=1 ./install.sh
# Явно задать домен: sudo pirate_DOMAIN=deploy.example.com ./install.sh  или  --domain deploy.example.com
# Каталог: распакованный pirate-linux-amd64/ (рядом с bin/, share/, install.sh).

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

usage() {
  echo "Использование: sudo $0 [--domain FQDN]" >&2
  echo "  Интерактивно спросит домен; Enter — без домена (UI по IP:80)." >&2
  echo "  Спрашивает имя и пароль веб-дашборда; Enter — имя admin, пароль случайный." >&2
  echo "  Без вопросов: sudo pirate_NONINTERACTIVE=1 $0" >&2
  echo "  Явно: sudo pirate_DOMAIN=FQDN $0" >&2
  echo "  Пользователь дашборда: pirate_UI_ADMIN_USERNAME, pirate_UI_ADMIN_PASSWORD" >&2
}

# Разбор --domain (флаг переопределяет pirate_DOMAIN из окружения).
while [[ $# -gt 0 ]]; do
  case "$1" in
    --domain)
      if [[ -z "${2:-}" ]]; then
        echo "Ошибка: --domain требует значение." >&2
        usage
        exit 1
      fi
      pirate_DOMAIN="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Неизвестный аргумент: $1" >&2
      usage
      exit 1
      ;;
  esac
done

# Убрать пробелы по краям
pirate_DOMAIN="${pirate_DOMAIN:-}"
pirate_DOMAIN="${pirate_DOMAIN#"${pirate_DOMAIN%%[![:space:]]*}"}"
pirate_DOMAIN="${pirate_DOMAIN%"${pirate_DOMAIN##*[![:space:]]}"}"

validate_domain() {
  local d="$1"
  [[ -n "$d" ]] || return 1
  [[ ${#d} -le 253 ]] || return 1
  case "$d" in
    */* | *:* | *\ *) return 1 ;;
  esac
  [[ "$d" != *..* ]] || return 1
  [[ "$d" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$ ]] || return 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

BIN_LOCAL="$SCRIPT_DIR/bin"
UI_SRC="$SCRIPT_DIR/share/ui/dist"
SYSTEMD_SRC="$SCRIPT_DIR/systemd"
NGINX_SRC="$SCRIPT_DIR/nginx"

for p in "$BIN_LOCAL/deploy-server" "$BIN_LOCAL/control-api" "$BIN_LOCAL/client" "$UI_SRC/index.html"; do
  if [[ ! -e "$p" ]]; then
    echo "Не найдено: $p — запускайте из распакованного архива pirate-linux-amd64." >&2
    exit 1
  fi
done

HOST_ARCH="$(uname -m)"
BIN_ARCH="$(file -b "$BIN_LOCAL/deploy-server" 2>/dev/null || true)"
if [[ "$HOST_ARCH" == "aarch64" ]] && [[ "$BIN_ARCH" == *"x86-64"* ]]; then
  echo "Ошибка: бинарники x86_64, а сервер ARM64. Соберите архив с TARGET_TRIPLE=aarch64-unknown-linux-gnu" >&2
  exit 1
fi

# Домен: уже задан через --domain / pirate_DOMAIN, иначе вопрос в TTY (Enter = только IP:порт для UI).
if [[ -z "$pirate_DOMAIN" ]] && [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  read -r -p "Домен для веб-интерфейса (Enter — пропустить, UI будет по http://<IP>:80/): " pirate_DOMAIN
  pirate_DOMAIN="${pirate_DOMAIN#"${pirate_DOMAIN%%[![:space:]]*}"}"
  pirate_DOMAIN="${pirate_DOMAIN%"${pirate_DOMAIN##*[![:space:]]}"}"
fi

if [[ -n "$pirate_DOMAIN" ]]; then
  if ! validate_domain "$pirate_DOMAIN"; then
    echo "Ошибка: недопустимое имя хоста: $pirate_DOMAIN" >&2
    exit 1
  fi
fi

# Первый пользователь веб-дашборда (Enter = имя admin, пароль — случайный)
_dash_name_def="admin"
_dash_pass_rand="$(openssl rand -base64 24 | tr -d '/+=' | head -c 24)"
if [[ -n "${pirate_UI_ADMIN_USERNAME:-}" ]]; then
  UI_ADMIN_NAME="${pirate_UI_ADMIN_USERNAME#"${pirate_UI_ADMIN_USERNAME%%[![:space:]]*}"}"
  UI_ADMIN_NAME="${UI_ADMIN_NAME%"${UI_ADMIN_NAME##*[![:space:]]}"}"
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  read -r -p "Имя пользователя веб-дашборда [${_dash_name_def}]: " _in_dash_name
  _in_dash_name="${_in_dash_name#"${_in_dash_name%%[![:space:]]*}"}"
  _in_dash_name="${_in_dash_name%"${_in_dash_name##*[![:space:]]}"}"
  UI_ADMIN_NAME="${_in_dash_name:-$_dash_name_def}"
else
  UI_ADMIN_NAME="$_dash_name_def"
fi
[[ -z "$UI_ADMIN_NAME" ]] && UI_ADMIN_NAME="$_dash_name_def"
if [[ ! "$UI_ADMIN_NAME" =~ ^[A-Za-z0-9._-]+$ ]] || [[ ${#UI_ADMIN_NAME} -gt 64 ]]; then
  echo "Ошибка: имя пользователя дашборда — только латинские буквы, цифры, . _ - (1–64 символа)." >&2
  exit 1
fi

if [[ -n "${pirate_UI_ADMIN_PASSWORD:-}" ]]; then
  UI_ADMIN_PASS="$pirate_UI_ADMIN_PASSWORD"
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  read -r -s -p "Пароль веб-дашборда (Enter — сгенерировать случайный): " _in_dash_pass
  echo ""
  if [[ -z "$_in_dash_pass" ]]; then
    UI_ADMIN_PASS="$_dash_pass_rand"
  else
    UI_ADMIN_PASS="$_in_dash_pass"
  fi
else
  UI_ADMIN_PASS="$_dash_pass_rand"
fi

CONTROL_API_JWT_SECRET_VALUE="$(openssl rand -base64 48 | tr -d '\n')"

echo "==> apt: nginx, postgresql, openssl"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq nginx postgresql openssl ca-certificates

echo "==> пользователь и каталоги"
if ! id deploy &>/dev/null; then
  useradd --system --create-home --home-dir /var/lib/pirate --shell /usr/sbin/nologin deploy
fi
install -d -o deploy -g deploy -m 0755 /var/lib/pirate/deploy
install -d -o deploy -g deploy -m 0755 /var/lib/pirate/ui

echo "==> бинарники -> /usr/local/bin"
install -m 0755 "$BIN_LOCAL/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_LOCAL/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_LOCAL/client" /usr/local/bin/client

echo "==> frontend -> /var/lib/pirate/ui/dist"
rm -rf /var/lib/pirate/ui/dist
cp -a "$UI_SRC" /var/lib/pirate/ui/dist
chown -R deploy:deploy /var/lib/pirate/ui

echo "==> PostgreSQL: пользователь и БД deploy"
# postgres не может chdir в cwd root (/root/...); избегаем предупреждений в логе
cd /
PG_PASS="${pirate_DB_PASSWORD:-$(openssl rand -base64 24 | tr -d '/+=' | head -c 32)}"
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

echo "==> /etc/pirate-deploy.env"
DATABASE_URL="postgresql://deploy:${PG_PASS}@127.0.0.1:5432/deploy"
pirate_PUBLIC_IP=""
if [[ -z "$pirate_DOMAIN" ]]; then
  pirate_PUBLIC_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
fi
umask 077
{
  cat <<EOF
DATABASE_URL=${DATABASE_URL}
DEPLOY_ROOT=/var/lib/pirate/deploy
GRPC_ENDPOINT=http://[::1]:50051
CONTROL_API_PORT=8080
RUST_LOG=info
CONTROL_UI_ADMIN_USERNAME=${UI_ADMIN_NAME}
CONTROL_UI_ADMIN_PASSWORD=${UI_ADMIN_PASS}
CONTROL_API_JWT_SECRET=${CONTROL_API_JWT_SECRET_VALUE}
EOF
  if [[ -n "$pirate_DOMAIN" ]]; then
    echo "DEPLOY_GRPC_PUBLIC_URL=http://${pirate_DOMAIN}:50051"
  else
    echo "DEPLOY_GRPC_PUBLIC_URL=http://${pirate_PUBLIC_IP:-127.0.0.1}:50051"
  fi
} >/etc/pirate-deploy.env
chmod 0640 /etc/pirate-deploy.env
chown root:deploy /etc/pirate-deploy.env

echo "==> systemd"
install -m 0644 "$SYSTEMD_SRC/deploy-server.service" /etc/systemd/system/deploy-server.service
install -m 0644 "$SYSTEMD_SRC/control-api.service" /etc/systemd/system/control-api.service
systemctl daemon-reload
systemctl enable deploy-server.service control-api.service

echo "==> nginx"
if [[ -n "$pirate_DOMAIN" ]]; then
  if [[ ! -f "$NGINX_SRC/nginx-pirate-site-domain.conf.in" ]]; then
    echo "Не найдено: $NGINX_SRC/nginx-pirate-site-domain.conf.in" >&2
    exit 1
  fi
  if [[ "$pirate_DOMAIN" == www.* ]]; then
    SERVER_NAMES="$pirate_DOMAIN"
  else
    SERVER_NAMES="$pirate_DOMAIN www.$pirate_DOMAIN"
  fi
  sed "s|__pirate_SERVER_NAMES__|${SERVER_NAMES}|g" \
    "$NGINX_SRC/nginx-pirate-site-domain.conf.in" >/etc/nginx/sites-available/pirate
  chmod 0644 /etc/nginx/sites-available/pirate
else
  install -m 0644 "$NGINX_SRC/nginx-pirate-site.conf" /etc/nginx/sites-available/pirate
fi
if [[ -f /etc/nginx/sites-enabled/default ]]; then
  rm -f /etc/nginx/sites-enabled/default
fi
ln -sf /etc/nginx/sites-available/pirate /etc/nginx/sites-enabled/pirate
nginx -t
systemctl enable nginx
systemctl restart nginx

echo "==> запуск сервисов"
systemctl restart postgresql
systemctl restart deploy-server.service
sleep 1

echo "==> gRPC: ключ control-api → deploy-server (reconcile / дашборд)"
sudo -u deploy env DEPLOY_ROOT=/var/lib/pirate/deploy /usr/local/bin/control-api bootstrap-grpc-key

GRPC_KEY_LINE='GRPC_SIGNING_KEY_PATH=/var/lib/pirate/deploy/.keys/control_api_ed25519.json'
if grep -q '^GRPC_SIGNING_KEY_PATH=' /etc/pirate-deploy.env 2>/dev/null; then
  sed -i "s|^GRPC_SIGNING_KEY_PATH=.*|${GRPC_KEY_LINE}|" /etc/pirate-deploy.env
else
  echo "$GRPC_KEY_LINE" >> /etc/pirate-deploy.env
fi
chmod 0640 /etc/pirate-deploy.env
chown root:deploy /etc/pirate-deploy.env

echo "==> перезапуск deploy-server (подхват authorized_peers после bootstrap)"
systemctl restart deploy-server.service
sleep 1
systemctl restart control-api.service

echo ""
echo "Готово."
echo "  Пароль БД (также в /etc/pirate-deploy.env): $PG_PASS"
echo "  Дашборд: логин ${UI_ADMIN_NAME}, пароль (также в /etc/pirate-deploy.env): $UI_ADMIN_PASS"
if [[ -n "$pirate_DOMAIN" ]]; then
  echo "  UI:        http://${pirate_DOMAIN}/"
  echo "  gRPC URL для pairing (также в /etc/pirate-deploy.env): http://${pirate_DOMAIN}:50051"
  echo "             Откройте порт 50051 в firewall, если клиенты подключаются не с этого хоста."
else
  _ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  echo "  UI:        http://${_ip}:80/"
  echo "             Домен не задан — доступ по IP и порту 80 (HTTP)."
fi
echo "  API health: curl -s http://127.0.0.1:8080/health"
echo "  Клиент: JSON для client pair (поля token, url, pairing):"
sudo -u deploy bash -c 'set -a; . /etc/pirate-deploy.env; set +a; exec /usr/local/bin/deploy-server --root /var/lib/pirate/deploy print-install-bundle'
echo "           Пример: client pair --bundle '<JSON выше>'   или сохраните JSON в файл и: client pair --bundle ./bundle.json"
echo "           Без pair команды client status / deploy вернут missing metadata (x-deploy-pubkey)."
echo "           После pair: client status / deploy (URL сохраняется из поля url в JSON)."
echo "  Проверка gRPC: не через curl к :50051 (это HTTP/2 gRPC); используйте client или grpcurl."
echo ""
echo "Логи: journalctl -u deploy-server -f   /   journalctl -u control-api -f"
