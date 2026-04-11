#!/usr/bin/env bash
# Удаление данных серверного клиента: артефакты деплоя, статика UI, ключи в DEPLOY_ROOT.
# Остановите службы заранее (sudo ./uninstall.sh), иначе файлы могут быть заняты.
#
# Запуск:
#   sudo ./purge-pirate-data.sh
#   sudo ./purge-pirate-data.sh --remove-postgres
#   sudo ./purge-pirate-data.sh --remove-postgres --remove-linux-user
#
# --remove-postgres  — DROP DATABASE/ROLE для legacy «deploy» и опционально explorer (pirate_explorer)
# --remove-linux-user  — удалить системного пользователя pirate (после удаления каталога)

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

REMOVE_PG=0
REMOVE_USER=0

usage() {
  echo "Использование: sudo $0 [--remove-postgres] [--remove-linux-user]" >&2
  echo "  Удаляет рекурсивно /var/lib/pirate (deploy-артефакты и UI)." >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --remove-postgres)
      REMOVE_PG=1
      shift
      ;;
    --remove-linux-user)
      REMOVE_USER=1
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

DATA_ROOT="/var/lib/pirate"

if [[ -d "$DATA_ROOT" ]] && command -v mountpoint >/dev/null 2>&1 && mountpoint -q "$DATA_ROOT" 2>/dev/null; then
  echo "Ошибка: $DATA_ROOT смонтирован как отдельная точка — удаление отменено." >&2
  exit 1
fi

if [[ "$REMOVE_PG" -eq 1 ]]; then
  if ! command -v psql >/dev/null 2>&1; then
    echo "Клиент psql не найден; пропуск --remove-postgres." >&2
    REMOVE_PG=0
  fi
fi

if [[ "$REMOVE_PG" -eq 1 ]]; then
  echo "==> PostgreSQL: legacy deploy / explorer pirate_explorer"
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='deploy'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP DATABASE IF EXISTS deploy;" >/dev/null
    echo "    База deploy удалена."
  else
    echo "    База deploy не найдена."
  fi
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='deploy'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP USER IF EXISTS deploy;" >/dev/null
    echo "    Роль deploy удалена."
  else
    echo "    Роль deploy не найдена."
  fi
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='pirate_explorer'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP DATABASE IF EXISTS pirate_explorer;" >/dev/null
    echo "    База pirate_explorer удалена."
  else
    echo "    База pirate_explorer не найдена."
  fi
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='pirate_explorer'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP USER IF EXISTS pirate_explorer;" >/dev/null
    echo "    Роль pirate_explorer удалена."
  else
    echo "    Роль pirate_explorer не найдена."
  fi
fi

echo "==> каталог данных: $DATA_ROOT"
if [[ -d "$DATA_ROOT" ]]; then
  rm -rf "$DATA_ROOT"
  echo "    Удалено."
else
  echo "    Нет каталога — пропуск."
fi

if [[ "$REMOVE_USER" -eq 1 ]]; then
  echo "==> системный пользователь pirate"
  if id pirate &>/dev/null; then
    userdel pirate 2>/dev/null || userdel -f pirate 2>/dev/null || {
      echo "Не удалось удалить пользователя pirate (возможно, заняты процессы)." >&2
      exit 1
    }
    echo "    Пользователь pirate удалён."
  else
    echo "    Пользователь pirate отсутствует."
  fi
  if id deploy &>/dev/null; then
    echo "    Предупреждение: остался legacy-пользователь deploy — удалите вручную при необходимости." >&2
  fi
fi

echo ""
echo "Готово."
if [[ "$REMOVE_PG" -eq 0 ]] || [[ "$REMOVE_USER" -eq 0 ]]; then
  echo "  Подсказка: для БД и пользователя ОС используйте --remove-postgres и/или --remove-linux-user"
fi
