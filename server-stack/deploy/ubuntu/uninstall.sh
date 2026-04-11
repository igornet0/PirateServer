#!/usr/bin/env bash
# Снятие установки серверного стека pirate: остановка служб, unit-файлы, nginx-сайт,
# бинарники в /usr/local/bin, /etc/pirate-deploy.env.
# Данные в /var/lib/pirate и PostgreSQL не трогает — см. purge-pirate-data.sh.
#
# Запуск: sudo ./uninstall.sh

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

usage() {
  echo "Использование: sudo $0" >&2
  echo "  Удаляет systemd-службы deploy-server, control-api (и tunnel-gateway, если есть)," >&2
  echo "  сайт nginx «pirate», бинарники и /etc/pirate-deploy.env." >&2
  echo "  Каталог данных /var/lib/pirate не удаляется — используйте purge-pirate-data.sh" >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
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

echo "==> остановка и отключение служб"
for s in deploy-server control-api tunnel-gateway; do
  systemctl stop "${s}.service" 2>/dev/null || true
  systemctl disable "${s}.service" 2>/dev/null || true
done

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
# install.sh удаляет default; если других сайтов нет — вернуть дефолтный nginx, чтобы reload не ломался.
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
echo "Готово: службы и файлы установки сняты."
echo "  Данные проектов и UI: /var/lib/pirate — удалить: sudo ./purge-pirate-data.sh"
