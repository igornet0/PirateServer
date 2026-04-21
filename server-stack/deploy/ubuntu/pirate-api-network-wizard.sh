#!/usr/bin/env bash
# Interactive: настроить доступ к control-api / дашборду (LAN vs публично), с учётом nginx.
# Правит /etc/pirate-deploy.env, при nginx — snippet + reload; перезапускает deploy-server и control-api.
#
# Неинтерактивно:
#   sudo PIRATE_NETWORK_WIZARD_NONINTERACTIVE=1 PIRATE_NETWORK_MODE=lan|public pirate-api-network-wizard.sh
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=pirate-env-common.sh
source "$HERE/pirate-env-common.sh"

ENVF="${PIRATE_DEPLOY_ENV:-/etc/pirate-deploy.env}"
SNIP="/etc/nginx/snippets/pirate-lan-access.conf"
SITE="/etc/nginx/sites-available/pirate"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

if [[ ! -f "$ENVF" ]]; then
  echo "Нет $ENVF — сначала установите стек (install.sh)." >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "Нужен python3 для правки $ENVF." >&2
  exit 1
fi

if [[ -x "$HERE/pirate-ensure-jwt-secret.sh" ]]; then
  "$HERE/pirate-ensure-jwt-secret.sh" "$ENVF" || true
fi

primary_ip() {
  local a
  a="$(hostname -I 2>/dev/null | awk '{print $1}')"
  if [[ -z "$a" ]]; then
    a="127.0.0.1"
  fi
  echo "$a"
}

http_host_for_grpc_url() {
  python3 - "$1" <<'PY'
import sys, urllib.parse
raw = (sys.argv[1] or "").strip()
if not raw:
    sys.exit(1)
u = urllib.parse.urlparse(raw)
h = u.hostname
if not h:
    sys.exit(1)
sch = "https" if u.scheme == "https" else "http"
print(f"{sch}://{h}")
PY
}

control_api_port() {
  local p
  p="$(pirate_env_get_raw "$ENVF" CONTROL_API_PORT 2>/dev/null || true)"
  if [[ -z "${p// }" ]] || ! [[ "$p" =~ ^[0-9]+$ ]]; then
    echo "8080"
  else
    echo "$p"
  fi
}

nginx_site_enabled() {
  [[ -f "$SITE" ]] && [[ -L /etc/nginx/sites-enabled/pirate ]] && command -v nginx >/dev/null 2>&1
}

nginx_daemon_running() {
  systemctl is-active --quiet nginx 2>/dev/null
}

write_nginx_snippet_lan() {
  mkdir -p "$(dirname "$SNIP")"
  cat >"$SNIP" <<'EOF'
# pirate-api-network-wizard: только локальные сети (RFC1918) + localhost
allow 127.0.0.1;
allow ::1;
allow 10.0.0.0/8;
allow 172.16.0.0/12;
allow 192.168.0.0/16;
deny all;
EOF
  chmod 0644 "$SNIP"
}

write_nginx_snippet_public() {
  mkdir -p "$(dirname "$SNIP")"
  cat >"$SNIP" <<'EOF'
# pirate-api-network-wizard: публичный режим (ограничений по IP в nginx нет)
EOF
  chmod 0644 "$SNIP"
}

ensure_nginx_include() {
  if [[ ! -f "$SITE" ]]; then
    return 0
  fi
  if grep -qF 'pirate-lan-access.conf' "$SITE" 2>/dev/null; then
    return 0
  fi
  if ! grep -qE '^[[:space:]]*server[[:space:]]*\{' "$SITE"; then
    echo "Предупреждение: в $SITE не найден блок server { — include не добавлен." >&2
    return 0
  fi
  sed -i '/^[[:space:]]*server[[:space:]]*{/a\    include /etc/nginx/snippets/pirate-lan-access.conf;' "$SITE"
}

reload_nginx_safe() {
  if ! nginx_site_enabled; then
    return 0
  fi
  nginx -t
  if nginx_daemon_running; then
    systemctl reload nginx
  else
    echo "nginx не запущен: sudo systemctl start nginx   (после правок конфигурации)" >&2
  fi
}

IP="$(primary_ip)"
PORT="$(control_api_port)"
MODE=""
NON="${PIRATE_NETWORK_WIZARD_NONINTERACTIVE:-}"

if [[ "$NON" == "1" ]]; then
  MODE="${PIRATE_NETWORK_MODE:-}"
  case "${MODE,,}" in
    lan|local|private) MODE="lan" ;;
    public|wan|internet) MODE="public" ;;
    *)
      echo "PIRATE_NETWORK_MODE должен быть lan или public" >&2
      exit 1
      ;;
  esac
else
  echo ""
  echo "Мастер сети Pirate — доступ к HTTP API и веб-дашборду"
  echo "Текущий IP (первый из hostname -I): $IP  порт control-api: $PORT"
  if nginx_site_enabled; then
    echo "Включён сайт nginx «pirate» — API за reverse-proxy (:80 → 127.0.0.1:$PORT)."
    if ! nginx_daemon_running; then
      echo "(Сервис nginx сейчас не активен — после настройки выполните: sudo systemctl start nginx)"
    fi
  else
    echo "Сайт nginx «pirate» не включён — клиенты ходят напрямую на control-api (обычно :$PORT), если bind разрешён."
  fi
  echo ""
  echo "Кому открыть доступ?"
  echo "  1 — только локальная сеть (частные адреса 10/8, 172.16/12, 192.168/16) и localhost"
  echo "  2 — также из интернета (публичный доступ по HTTP; для прод лучше TLS и домен)"
  read -r -p "Выбор [1/2] (по умолчанию 1): " _choice
  _choice="${_choice#"${_choice%%[![:space:]]*}"}"
  _choice="${_choice%"${_choice##*[![:space:]]}"}"
  case "${_choice:-1}" in
    2|2.|п|П|public|интернет)
      MODE="public"
      ;;
    *)
      MODE="lan"
      ;;
  esac
fi

GRPC_LINE="$(pirate_env_get_raw "$ENVF" DEPLOY_GRPC_PUBLIC_URL 2>/dev/null || true)"
CTRL_PUBLIC_SUGGEST="http://${IP}"
if [[ -n "${GRPC_LINE// }" ]]; then
  if _h="$(http_host_for_grpc_url "$GRPC_LINE" 2>/dev/null)"; then
    CTRL_PUBLIC_SUGGEST="$_h"
  fi
fi

if nginx_site_enabled; then
  pirate_env_upsert "$ENVF" CONTROL_API_BIND "127.0.0.1"
  pirate_env_upsert "$ENVF" DEPLOY_CONTROL_API_DIRECT_URL "http://127.0.0.1:${PORT}"
  pirate_env_upsert "$ENVF" DEPLOY_CONTROL_API_PUBLIC_URL "$CTRL_PUBLIC_SUGGEST"
  if [[ "$MODE" == "lan" ]]; then
    write_nginx_snippet_lan
    ensure_nginx_include
    reload_nginx_safe
    echo "nginx: ограничение по частным сетям записано в $SNIP"
  else
    write_nginx_snippet_public
    ensure_nginx_include
    reload_nginx_safe
    echo "nginx: публичный режим ($SNIP без deny all)"
  fi
  if [[ "$MODE" == "public" ]]; then
    echo "Внимание: публичный HTTP без TLS — для прод настройте HTTPS (certbot / свой прокси)." >&2
  fi
else
  pirate_env_upsert "$ENVF" CONTROL_API_BIND "0.0.0.0"
  pirate_env_upsert "$ENVF" DEPLOY_CONTROL_API_DIRECT_URL "http://127.0.0.1:${PORT}"
  pirate_env_upsert "$ENVF" DEPLOY_CONTROL_API_PUBLIC_URL "http://${IP}:${PORT}"
  if [[ "$MODE" == "lan" ]]; then
    echo "Без nginx: control-api слушает 0.0.0.0:${PORT}. Ограничьте доступ firewall (например UFW) только для LAN."
    echo "  Пример: sudo ufw allow from 192.168.0.0/16 to any port ${PORT} proto tcp"
    echo "  Порт gRPC для клиентов (pair): откройте 50051/tcp для тех же сетей."
  else
    echo "Без nginx: публичный доступ на 0.0.0.0:${PORT} — настройте firewall и по возможности TLS на отдельном прокси." >&2
  fi
fi

if [[ -z "${GRPC_LINE// }" ]]; then
  pirate_env_upsert "$ENVF" DEPLOY_GRPC_PUBLIC_URL "http://${IP}:50051"
  echo "Добавлен DEPLOY_GRPC_PUBLIC_URL=http://${IP}:50051 (gRPC для pair / клиентов)."
fi

echo "Перезапуск deploy-server и control-api…"
pirate_restart_stack_services

echo ""
echo "Готово. DEPLOY_CONTROL_API_PUBLIC_URL=$(pirate_env_get_raw "$ENVF" DEPLOY_CONTROL_API_PUBLIC_URL)"
CTRL_PUBLIC="$(pirate_env_get_raw "$ENVF" DEPLOY_CONTROL_API_PUBLIC_URL)"
echo "Проверка с клиента в той же сети: curl -sS \"${CTRL_PUBLIC}/health\""
echo "Проверка на сервере (слушает ли ${PORT}/tcp): ss -tlnp | grep -E \":${PORT}\\b\" || true"
echo "Если таймаут с клиента — откройте ${PORT}/tcp в firewall для LAN (см. подсказки выше для UFW)."
