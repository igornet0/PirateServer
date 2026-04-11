#!/usr/bin/env bash
# Снятие установки серверного стека Pirate.
#
# По умолчанию — полное удаление: службы, данные /var/lib/pirate, пользователь pirate,
# PostgreSQL (роли/БД deploy и pirate_explorer), helper-скрипты /usr/local/lib/pirate,
# sudoers для SMB, бинарники, nginx-сайт, /etc/pirate-deploy.env.
#
# Минимально (только службы и файлы установки, данные не трогать):
#   sudo ./uninstall.sh --services-only
#
# Дополнительно удалить каталог распаковки архива (после завершения скрипта, в фоне):
#   sudo ./uninstall.sh --remove-bundle-dir
#
# Запуск: sudo ./uninstall.sh [опции]

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SERVICES_ONLY=0
REMOVE_BUNDLE_DIR=0

usage() {
  echo "Использование: sudo $0 [опции]" >&2
  echo "" >&2
  echo "  По умолчанию — полное удаление со всего диска следов установки (кроме пакетов apt," >&2
  echo "  если вы ставили nginx/postgresql/cifs вручную для других сервисов)." >&2
  echo "" >&2
  echo "Опции:" >&2
  echo "  --services-only     Только остановить службы, убрать unit-файлы, nginx «pirate»," >&2
  echo "                      бинарники в /usr/local/bin и /etc/pirate-deploy.env." >&2
  echo "                      Каталог /var/lib/pirate, пользователь pirate, PostgreSQL и" >&2
  echo "                      /usr/local/lib/pirate не удаляются." >&2
  echo "  --remove-bundle-dir После успешного завершения удалить каталог распаковки (где лежит" >&2
  echo "                      этот скрипт). Выполняется с задержкой в фоне." >&2
  echo "  -h, --help          Справка." >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --services-only)
      SERVICES_ONLY=1
      shift
      ;;
    --remove-bundle-dir)
      REMOVE_BUNDLE_DIR=1
      shift
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

if [[ "$REMOVE_BUNDLE_DIR" == "1" ]] && [[ "$SERVICES_ONLY" == "1" ]]; then
  echo "Ошибка: --remove-bundle-dir несовместим с --services-only (данные не очищены)." >&2
  exit 1
fi

echo "==> остановка и отключение служб"
for s in deploy-server control-api tunnel-gateway; do
  systemctl stop "${s}.service" 2>/dev/null || true
  systemctl disable "${s}.service" 2>/dev/null || true
done

if [[ "$SERVICES_ONLY" != "1" ]]; then
  echo "==> данные, PostgreSQL, пользователь ОС (purge)"
  if [[ -x "$SCRIPT_DIR/purge-pirate-data.sh" ]]; then
    bash "$SCRIPT_DIR/purge-pirate-data.sh" --remove-postgres --remove-linux-user
  else
    echo "Предупреждение: purge-pirate-data.sh не найден в $SCRIPT_DIR — удаляю вручную." >&2
    DATA_ROOT="/var/lib/pirate"
    if [[ -d "$DATA_ROOT" ]] && command -v mountpoint >/dev/null 2>&1 && mountpoint -q "$DATA_ROOT" 2>/dev/null; then
      echo "Ошибка: $DATA_ROOT смонтирован отдельно — удаление отменено." >&2
      exit 1
    fi
    if command -v psql >/dev/null 2>&1; then
      for db in deploy pirate_explorer; do
        sudo -u postgres psql -c "DROP DATABASE IF EXISTS ${db};" 2>/dev/null || true
      done
      for role in deploy pirate_explorer; do
        sudo -u postgres psql -c "DROP USER IF EXISTS ${role};" 2>/dev/null || true
      done
    fi
    if [[ -d "$DATA_ROOT" ]]; then
      rm -rf "$DATA_ROOT"
    fi
    if id pirate &>/dev/null; then
      userdel -f pirate 2>/dev/null || userdel pirate 2>/dev/null || true
    fi
    if id deploy &>/dev/null; then
      userdel -f deploy 2>/dev/null || userdel deploy 2>/dev/null || true
    fi
  fi

  echo "==> sudoers и /usr/local/lib/pirate"
  rm -f /etc/sudoers.d/99-pirate-smb
  rm -rf /usr/local/lib/pirate
fi

echo "==> удаление unit-файлов"
for u in deploy-server.service control-api.service tunnel-gateway.service; do
  rm -f "/etc/systemd/system/$u"
done
systemctl daemon-reload

echo "==> nginx: сайт pirate"
if [[ -L /etc/nginx/sites-enabled/pirate ]]; then
  rm -f /etc/nginx/sites-enabled/pirate
fi
rm -f /etc/nginx/sites-available/pirate
shopt -s nullglob
enabled=(/etc/nginx/sites-enabled/*)
shopt -u nullglob
if [[ ${#enabled[@]} -eq 0 ]] && [[ -f /etc/nginx/sites-available/default ]]; then
  ln -sf /etc/nginx/sites-available/default /etc/nginx/sites-enabled/default
  echo "    Восстановлен /etc/nginx/sites-enabled/default"
fi
if command -v nginx >/dev/null 2>&1; then
  if nginx -t 2>/dev/null; then
    systemctl reload nginx 2>/dev/null || true
  else
    echo "Внимание: nginx -t завершился с ошибкой — проверьте конфигурацию вручную." >&2
  fi
fi

echo "==> бинарники"
for b in deploy-server control-api client tunnel-gateway; do
  if [[ -f "/usr/local/bin/$b" ]]; then
    rm -f "/usr/local/bin/$b"
  fi
done

echo "==> конфиг окружения"
rm -f /etc/pirate-deploy.env

echo ""
if [[ "$SERVICES_ONLY" == "1" ]]; then
  echo "Готово: сняты только службы и файлы установки."
  echo "  Полное удаление данных и пользователя: sudo $0   (без --services-only)"
else
  echo "Готово: полное удаление следов Pirate (данные, пользователь pirate, helper-скрипты, sudoers)."
  echo "  Пакеты apt (nginx, postgresql, cifs-utils и т.д.) не удалялись — при необходимости: apt remove …"
fi

if [[ "$REMOVE_BUNDLE_DIR" == "1" ]]; then
  case "$SCRIPT_DIR" in
    / | /usr | /usr/* | /etc | /etc/* | /bin | /bin/* | /sbin | /sbin/*)
      echo "Ошибка: отказ удалить небезопасный путь: $SCRIPT_DIR" >&2
      exit 1
      ;;
  esac
  echo ""
  echo "==> отложенное удаление каталога распаковки: $SCRIPT_DIR"
  # Удаление после выхода процесса: иначе удаляется открытый скрипт.
  nohup bash -c "sleep 1; rm -rf \"${SCRIPT_DIR}\"" >/dev/null 2>&1 &
  echo "    Запущено в фоне (через ~1 с). Проверьте: ls $(dirname "$SCRIPT_DIR")"
fi
