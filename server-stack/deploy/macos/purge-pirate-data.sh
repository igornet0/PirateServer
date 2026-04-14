#!/usr/bin/env bash
# Удаление данных Pirate на macOS.
#
#   sudo ./purge-pirate-data.sh
#   sudo ./purge-pirate-data.sh --remove-postgres   (если postgres вручную; как на Linux)
#   sudo ./purge-pirate-data.sh --remove-postgres --remove-os-user

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Этот скрипт только для macOS." >&2
  exit 1
fi

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

REMOVE_PG=0
REMOVE_USER=0

usage() {
  echo "Использование: sudo $0 [--remove-postgres] [--remove-os-user]" >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --remove-postgres) REMOVE_PG=1; shift ;;
    --remove-os-user) REMOVE_USER=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Неизвестный аргумент: $1" >&2; usage; exit 1 ;;
  esac
done

DATA_ROOT="/var/lib/pirate"

if [[ -d "$DATA_ROOT" ]] && command -v diskutil >/dev/null 2>&1; then
  if diskutil info "$DATA_ROOT" 2>/dev/null | grep -q "Volume Name"; then
    : # not a separate mount check like Linux mountpoint
  fi
fi

if [[ "$REMOVE_PG" -eq 1 ]]; then
  if ! command -v psql >/dev/null 2>&1; then
    echo "Клиент psql не найден; пропуск --remove-postgres." >&2
    REMOVE_PG=0
  fi
fi

if [[ "$REMOVE_PG" -eq 1 ]]; then
  echo "==> PostgreSQL (если установлен локально)"
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='deploy'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP DATABASE IF EXISTS deploy;" >/dev/null || true
  fi
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='deploy'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP USER IF EXISTS deploy;" >/dev/null || true
  fi
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='pirate_explorer'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP DATABASE IF EXISTS pirate_explorer;" >/dev/null || true
  fi
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='pirate_explorer'" 2>/dev/null | grep -q 1; then
    sudo -u postgres psql -c "DROP USER IF EXISTS pirate_explorer;" >/dev/null || true
  fi
fi

echo "==> каталог данных: $DATA_ROOT"
if [[ -d "$DATA_ROOT" ]]; then
  rm -rf "$DATA_ROOT"
  echo "    Удалено."
else
  echo "    Нет каталога."
fi

if [[ "$REMOVE_USER" -eq 1 ]]; then
  echo "==> пользователь pirate (macOS)"
  if id pirate &>/dev/null; then
    dseditgroup -o edit -d pirate -t user pirate 2>/dev/null || true
    dscl . -delete /Users/pirate 2>/dev/null || true
    echo "    Запись пользователя pirate удалена."
  else
    echo "    Пользователь pirate отсутствует."
  fi
fi

echo ""
echo "Готово."
