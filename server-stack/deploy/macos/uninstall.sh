#!/usr/bin/env bash
# Снятие установки Pirate на macOS (launchd, бинарники, nginx-фрагмент Homebrew).
#
#   sudo ./uninstall.sh                  — полное удаление (данные, пользователь pirate, helper-скрипты)
#   sudo ./uninstall.sh --services-only — только службы и файлы установки
#
# Запуск: sudo ./uninstall.sh [опции]

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Этот скрипт только для macOS." >&2
  exit 1
fi

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SERVICES_ONLY=0
REMOVE_BUNDLE_DIR=0
BUNDLE_DIR_EXPLICIT=""

usage() {
  echo "Использование: sudo $0 [опции]" >&2
  echo "  --services-only — только launchd, бинарники, env, nginx pirate.conf" >&2
  echo "  --remove-bundle-dir[=PATH] — удалить каталог распаковки (фон, через ~1 с)" >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --services-only) SERVICES_ONLY=1; shift ;;
    --remove-bundle-dir) REMOVE_BUNDLE_DIR=1; shift ;;
    --remove-bundle-dir=*) REMOVE_BUNDLE_DIR=1; BUNDLE_DIR_EXPLICIT="${1#*=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Неизвестный аргумент: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ "$REMOVE_BUNDLE_DIR" == "1" ]] && [[ "$SERVICES_ONLY" == "1" ]]; then
  echo "Ошибка: --remove-bundle-dir несовместим с --services-only." >&2
  exit 1
fi

BUNDLE_DIR_TO_REMOVE=""
if [[ "$REMOVE_BUNDLE_DIR" == "1" ]]; then
  if [[ -n "$BUNDLE_DIR_EXPLICIT" ]]; then
    BUNDLE_DIR_TO_REMOVE="$BUNDLE_DIR_EXPLICIT"
  elif [[ -f /var/lib/pirate/original-bundle-path ]]; then
    BUNDLE_DIR_TO_REMOVE="$(cat /var/lib/pirate/original-bundle-path)"
  else
    BUNDLE_DIR_TO_REMOVE="$SCRIPT_DIR"
  fi
  case "${BUNDLE_DIR_TO_REMOVE}" in
    /*) ;;
    *) echo "Ошибка: путь должен быть абсолютным." >&2; exit 1 ;;
  esac
  case "${BUNDLE_DIR_TO_REMOVE}" in
    /|/usr|/usr/*|/etc|/etc/*|/bin|/bin/*|/sbin|/sbin/*)
      echo "Ошибка: небезопасный путь: ${BUNDLE_DIR_TO_REMOVE}" >&2
      exit 1
      ;;
  esac
fi

echo "==> остановка launchd"
for domain in system; do
  launchctl bootout "$domain" /Library/LaunchDaemons/com.pirate.deploy-server.plist 2>/dev/null || true
  launchctl bootout "$domain" /Library/LaunchDaemons/com.pirate.control-api.plist 2>/dev/null || true
done
rm -f /Library/LaunchDaemons/com.pirate.deploy-server.plist /Library/LaunchDaemons/com.pirate.control-api.plist

if [[ "$SERVICES_ONLY" != "1" ]]; then
  echo "==> данные и пользователь"
  if [[ -x "$SCRIPT_DIR/purge-pirate-data.sh" ]]; then
    bash "$SCRIPT_DIR/purge-pirate-data.sh" --remove-postgres --remove-os-user
  else
    echo "Предупреждение: purge-pirate-data.sh не найден." >&2
    rm -rf /var/lib/pirate
  fi

  echo "==> sudoers и /usr/local/lib/pirate"
  rm -f /etc/sudoers.d/99-pirate-smb
  rm -rf /usr/local/lib/pirate
fi

echo "==> nginx (Homebrew servers/pirate.conf)"
if command -v brew >/dev/null 2>&1 || [[ -x /opt/homebrew/bin/brew ]] || [[ -x /usr/local/bin/brew ]]; then
  BREW="$(command -v brew 2>/dev/null || echo /opt/homebrew/bin/brew)"
  [[ -x "$BREW" ]] || BREW="/usr/local/bin/brew"
  if [[ -x "$BREW" ]]; then
    NGINX_PREFIX="$("$BREW" --prefix nginx 2>/dev/null)" || true
    if [[ -n "$NGINX_PREFIX" ]] && [[ -f "$NGINX_PREFIX/etc/nginx/servers/pirate.conf" ]]; then
      rm -f "$NGINX_PREFIX/etc/nginx/servers/pirate.conf"
    fi
    if [[ -x "$NGINX_PREFIX/bin/nginx" ]]; then
      "$NGINX_PREFIX/bin/nginx" -s reload 2>/dev/null || true
    fi
  fi
fi

echo "==> бинарники и libexec"
for b in deploy-server control-api pirate client tunnel-gateway; do
  rm -f "/usr/local/bin/$b"
done
rm -rf /usr/local/libexec/pirate

echo "==> конфиг окружения"
rm -f /etc/pirate-deploy.env

echo ""
if [[ "$SERVICES_ONLY" == "1" ]]; then
  echo "Готово: сняты службы и файлы установки."
else
  echo "Готово: полное удаление следов Pirate (кроме пакетов Homebrew)."
fi

if [[ "$REMOVE_BUNDLE_DIR" == "1" ]]; then
  echo "==> отложенное удаление: $BUNDLE_DIR_TO_REMOVE"
  nohup bash -c "sleep 1; rm -rf \"${BUNDLE_DIR_TO_REMOVE}\"" >/dev/null 2>&1 &
fi

if [[ "$SERVICES_ONLY" != "1" ]]; then
  if [[ -d /usr/local/share/pirate-uninstall ]]; then
    rm -rf /usr/local/share/pirate-uninstall
  fi
fi
