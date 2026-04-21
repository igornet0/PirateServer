#!/usr/bin/env bash
# Add dashboard user (Argon2 in DB) + ensure JWT secret + restart control-api so login works.
# Usage:
#   sudo pirate-dashboard-add-user.sh USER PASS
#   sudo pirate-dashboard-add-user.sh   # interactive (username + password)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=pirate-env-common.sh
source "$HERE/pirate-env-common.sh"

ENVF="${PIRATE_DEPLOY_ENV:-/etc/pirate-deploy.env}"
DEPLOY_SERVER="${DEPLOY_SERVER:-/usr/local/bin/deploy-server}"
DEPLOY_ROOT="${DEPLOY_ROOT:-/var/lib/pirate/deploy}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0 $*" >&2
  exit 1
fi

if [[ ! -f "$ENVF" ]]; then
  echo "Нет $ENVF — сначала установите стек (install.sh)." >&2
  exit 1
fi

if [[ ! -x "$DEPLOY_SERVER" ]]; then
  echo "Не найден $DEPLOY_SERVER" >&2
  exit 1
fi

"$HERE/pirate-ensure-jwt-secret.sh" "$ENVF"

user="${1:-}"
pass="${2:-}"
if [[ -z "$user" ]]; then
  read -r -p "Имя пользователя дашборда: " user
fi
user="${user#"${user%%[![:space:]]*}"}"
user="${user%"${user##*[![:space:]]}"}"
if [[ -z "$user" ]]; then
  echo "Имя пользователя не задано." >&2
  exit 1
fi

if [[ -z "$pass" ]]; then
  read -r -s -p "Пароль: " pass
  echo ""
fi
if [[ -z "$pass" ]]; then
  echo "Пароль не задан." >&2
  exit 1
fi

sudo -u pirate bash -s "$ENVF" "$DEPLOY_SERVER" "$DEPLOY_ROOT" "$user" "$pass" <<'EOS'
set -euo pipefail
set -a
source "$1"
set +a
exec "$2" --root "$3" dashboard-add-user --username "$4" --password "$5"
EOS

echo "Перезапуск control-api (подхват CONTROL_API_JWT_SECRET и сессии JWT)…"
pirate_restart_control_api_only
echo "Готово: пользователь «$user» добавлен; вход в дашборд доступен при настроенном DEPLOY_CONTROL_API_PUBLIC_URL и сети."
