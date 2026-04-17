#!/usr/bin/env bash
# Управляет nginx: установка/настройка сайта Pirate или удаление пакета.
set -euo pipefail
MODE="${1:-api_only}"
if [[ "$MODE" != "api_only" && "$MODE" != "with_ui" && "$MODE" != "remove" ]]; then
  echo "usage: pirate-ensure-nginx.sh [api_only|with_ui|remove]" >&2
  exit 1
fi

if [[ "$MODE" == "remove" ]]; then
  if command -v systemctl >/dev/null 2>&1; then
    systemctl stop nginx 2>/dev/null || true
    systemctl disable nginx 2>/dev/null || true
  fi
  rm -f /etc/nginx/sites-enabled/pirate /etc/nginx/sites-available/pirate
  export DEBIAN_FRONTEND=noninteractive
  apt-get purge -y -qq nginx nginx-common || true
  apt-get autoremove -y -qq || true
  echo "ok: nginx removed"
  exit 0
fi

if ! command -v nginx >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq nginx openssl ca-certificates
fi

mkdir -p /etc/nginx/sites-available /etc/nginx/sites-enabled
SITE=/etc/nginx/sites-available/pirate

if [[ "$MODE" == "with_ui" ]]; then
  cat >"$SITE" <<'NGX_PIRATE_SITE'
# Pirate: UI + /api/ (control-api на 127.0.0.1:8080)
server {
    listen 80 default_server;
    listen [::]:80 default_server;

    root /var/lib/pirate/ui/dist;
    index index.html;

    location / {
        try_files $uri $uri/ /index.html;
    }

    location /api/ {
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_pass http://127.0.0.1:8080;
    }

    location /health {
        proxy_pass http://127.0.0.1:8080/health;
    }
}
NGX_PIRATE_SITE
else
  cat >"$SITE" <<'NGX_PIRATE_API'
# Pirate: только прокси /api/ на control-api (без статики UI)
server {
    listen 80 default_server;
    listen [::]:80 default_server;

    location /api/ {
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_pass http://127.0.0.1:8080;
    }

    location /health {
        proxy_pass http://127.0.0.1:8080/health;
    }

    location / {
        return 404;
    }
}
NGX_PIRATE_API
fi

chmod 0644 "$SITE"
if [[ -f /etc/nginx/sites-enabled/default ]]; then
  rm -f /etc/nginx/sites-enabled/default
fi
ln -sf "$SITE" /etc/nginx/sites-enabled/pirate
nginx -t
systemctl enable nginx 2>/dev/null || true
systemctl restart nginx
echo "ok: nginx ensured mode=$MODE site=$SITE"
