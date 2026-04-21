#!/usr/bin/env bash
# Установка на Ubuntu (x86_64 или ARM64 при наличии нативных бинарников в комплекте).
#
# Компоненты (Makefile: install-clear / install-nginx / install-ui / install-all):
#   sudo ./install.sh                      — только backend (SQLite метаданные, systemd, бинарники), без nginx и без статики UI
#   sudo ./install.sh --nginx              — + nginx (прокси /api на control-api; без --ui статики нет)
#   sudo ./install.sh --ui                 — + копирование share/ui/dist (без nginx веб на :80 не настраивается)
#   sudo ./install.sh --nginx --ui         — полный стек: nginx + статика дашборда
#
# Переменные окружения (до запуска или в оболочке): pirate_NGINX=0|1, pirate_UI=0|1 — то же, что флаги.
# С --ui: интерактивно спросит домен (Enter — без домена, доступ по IP) и учётную запись веб-дашборда;
#   в /etc/pirate-deploy.env пишутся CONTROL_UI_* и CONTROL_API_JWT_SECRET.
# Без --ui эти вопросы не задаются; JWT и учётка не записываются (клиенту достаточно pair по JSON).
# install.sh всегда задаёт CONTROL_API_BIND=127.0.0.1 (control-api только на localhost; nginx проксирует на 127.0.0.1:8080).
# Без вопроса (автоматизация): sudo pirate_NONINTERACTIVE=1 ./install.sh
# OTA server-stack: интерактивно спросит DEPLOY_ALLOW_SERVER_STACK_UPDATE; иначе pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE=0|1
# Статистика хостов в дашборде: CONTROL_API_HOST_STATS_SERIES / CONTROL_API_HOST_STATS_STREAM; см. pirate_CONTROL_API_HOST_STATS_* в usage.
# Явно задать домен: sudo pirate_DOMAIN=deploy.example.com ./install.sh  или  --domain deploy.example.com
# Каталог: распакованный pirate-linux-amd64/ (рядом с bin/, share/, install.sh).
# Состав бандла (см. scripts/linux-bundle-build.sh): bin/{deploy-server,control-api,client,pirate},
#   systemd/*.service, nginx/*.conf*, lib/pirate/*.sh и 99-pirate-smb.sudoers.fragment, bin/pirate-host-agent (если есть в архиве), share/ui/dist (если не .bundle-no-ui),
#   server-stack-manifest.json, env.example.
# Блок «GUI / трансляция»: сначала bin/pirate (или bin/client) gui-check из бандла, иначе pirate-gui-probe.sh;
#   затем вопрос о display stream при gui_detected; см. PIRATE_DISPLAY_STREAM_CONSENT в /etc/pirate-deploy.env
#   и /var/lib/pirate/host-gui-install.json.
# После установки в PATH: client и pirate (симлинк на client) — gRPC CLI к deploy-server на этом хосте.
# Если в каталоге есть .bundle-no-ui (архив собран с UI_BUILD=0), флаги --ui и pirate_UI=1 запрещены.

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "Запустите с sudo: sudo $0" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NO_UI_MARKER="$SCRIPT_DIR/.bundle-no-ui"

usage() {
  echo "Использование: sudo $0 [--domain FQDN] [--nginx] [--ui]" >&2
  echo "  По умолчанию — только сервисы (без nginx и без статики UI)." >&2
  echo "  Полный веб-стек: sudo $0 --nginx --ui  (или pirate_NGINX=1 pirate_UI=1 $0)" >&2
  if [[ -f "$NO_UI_MARKER" ]]; then
    echo "  Этот архив без UI (.bundle-no-ui): --ui и pirate_UI=1 недопустимы." >&2
  fi
  echo "  С --ui: интерактивно спросит домен; Enter — без домена (при --nginx — по IP:80)." >&2
  echo "  С --ui: спросит имя и пароль веб-дашборда; Enter — имя admin, пароль случайный." >&2
  echo "  Без --ui домен и учётная запись не запрашиваются (при необходимости — --domain / env)." >&2
  echo "  Без вопросов: sudo pirate_NONINTERACTIVE=1 $0" >&2
  echo "  Явно: sudo pirate_DOMAIN=FQDN $0 [--nginx] [--ui]" >&2
  echo "  С --ui: пользователь дашборда — pirate_UI_ADMIN_USERNAME, pirate_UI_ADMIN_PASSWORD" >&2
  echo "  OTA server-stack: pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE=0|1 (без вопроса; иначе спросит в TTY)" >&2
  echo "  Статистика хостов (дашборд): pirate_CONTROL_API_HOST_STATS_SERIES=0|1, pirate_CONTROL_API_HOST_STATS_STREAM=0|1" >&2
  echo "  Трансляция экрана: pirate_DISPLAY_STREAM_CONSENT=0|1 (без вопроса в TTY; иначе вопрос если bin/pirate gui-check дал gui_detected)" >&2
  echo "  Опционально: pirate_INSTALL_CIFS=1; pirate_INSTALL_POSTGRESQL=1; pirate_INSTALL_MYSQL=1;" >&2
  echo "    pirate_INSTALL_REDIS=1; pirate_INSTALL_MONGODB=1; pirate_INSTALL_MSSQL=1;" >&2
  echo "    pirate_INSTALL_CLICKHOUSE=1; pirate_INSTALL_ORACLE_NOTES=1 (установка СУБД на хост)." >&2
}

# Разбор --domain (флаг переопределяет pirate_DOMAIN из окружения).
while [[ $# -gt 0 ]]; do
  case "$1" in
    --nginx)
      pirate_NGINX=1
      shift
      ;;
    --ui)
      if [[ -f "$NO_UI_MARKER" ]]; then
        echo "Ошибка: архив собран без статики дашборда (UI_BUILD=0); флаг --ui недопустим." >&2
        exit 1
      fi
      pirate_UI=1
      shift
      ;;
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

# Значения по умолчанию: без флагов и без env — минимальная установка (см. Makefile install-clear).
pirate_NGINX="${pirate_NGINX:-0}"
pirate_UI="${pirate_UI:-0}"
if [[ -f "$NO_UI_MARKER" ]] && [[ "$pirate_UI" == "1" ]]; then
  echo "Ошибка: архив собран без статики дашборда (UI_BUILD=0); pirate_UI=1 недопустим." >&2
  exit 1
fi

pirate_INSTALL_CIFS="${pirate_INSTALL_CIFS:-0}"
pirate_INSTALL_POSTGRESQL="${pirate_INSTALL_POSTGRESQL:-0}"
pirate_INSTALL_MYSQL="${pirate_INSTALL_MYSQL:-0}"
pirate_INSTALL_REDIS="${pirate_INSTALL_REDIS:-0}"
pirate_INSTALL_MONGODB="${pirate_INSTALL_MONGODB:-0}"
pirate_INSTALL_MSSQL="${pirate_INSTALL_MSSQL:-0}"
pirate_INSTALL_CLICKHOUSE="${pirate_INSTALL_CLICKHOUSE:-0}"
pirate_INSTALL_ORACLE_NOTES="${pirate_INSTALL_ORACLE_NOTES:-0}"

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

cd "$SCRIPT_DIR"

BIN_LOCAL="$SCRIPT_DIR/bin"
UI_SRC="$SCRIPT_DIR/share/ui/dist"
SYSTEMD_SRC="$SCRIPT_DIR/systemd"
if [[ -d "$SCRIPT_DIR/nginx" ]]; then
  NGINX_SRC="$SCRIPT_DIR/nginx"
else
  NGINX_SRC="$SCRIPT_DIR"
fi

for p in "$BIN_LOCAL/deploy-server" "$BIN_LOCAL/control-api" "$BIN_LOCAL/client"; do
  if [[ ! -e "$p" ]]; then
    echo "Не найдено: $p — запускайте из распакованного архива pirate-linux-amd64." >&2
    exit 1
  fi
done
if [[ "$pirate_UI" == "1" ]] && [[ ! -e "$UI_SRC/index.html" ]]; then
  echo "Не найдено: $UI_SRC/index.html — для --ui нужен архив со статикой UI (share/ui/dist)." >&2
  exit 1
fi

HOST_ARCH="$(uname -m)"
BIN_ARCH="$(file -b "$BIN_LOCAL/deploy-server" 2>/dev/null || true)"
if [[ "$HOST_ARCH" == "aarch64" ]] && [[ "$BIN_ARCH" == *"x86-64"* ]]; then
  echo "Ошибка: бинарники x86_64, а сервер ARM64. Соберите архив с TARGET_TRIPLE=aarch64-unknown-linux-gnu" >&2
  exit 1
fi

# Домен: уже задан через --domain / pirate_DOMAIN; иначе вопрос в TTY только при --ui (для API-only/nginx без UI не спрашиваем).
if [[ "$pirate_UI" == "1" ]] && [[ -z "$pirate_DOMAIN" ]] && [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
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

# JWT и первый пользователь дашборда — только при --ui (без UI не пишем в env и не сидим в SQLite).
CONTROL_API_JWT_SECRET_VALUE=""
UI_ADMIN_NAME=""
UI_ADMIN_PASS=""
if [[ "$pirate_UI" == "1" ]]; then
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
fi

# DEPLOY_ALLOW_SERVER_STACK_UPDATE: gRPC UploadServerStack (sudoers уже содержит pirate-apply-stack-bundle.sh).
DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0"
if [[ -n "${pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE:-}" ]]; then
  case "${pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE,,}" in
    1|true|yes|y) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="1" ;;
    0|false|no|n) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0" ;;
    *)
      echo "Ошибка: pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE — ожидается 0/1, true/false, yes/no, y/n." >&2
      exit 1
      ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  read -r -p "Разрешить OTA-обновление server-stack через gRPC (DEPLOY_ALLOW_SERVER_STACK_UPDATE)? [y/N]: " _allow_stack
  _allow_stack="${_allow_stack#"${_allow_stack%%[![:space:]]*}"}"
  _allow_stack="${_allow_stack%"${_allow_stack##*[![:space:]]}"}"
  case "${_allow_stack,,}" in
    y|yes|1|true|д|да) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="1" ;;
    ""|n|no|0|false) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0" ;;
    *)
      echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
      exit 1
      ;;
  esac
fi

# CONTROL_API_HOST_STATS_*: история для графиков и SSE-телеметрия агентов (нагрузка на слабом сервере может быть заметной).
CONTROL_API_HOST_STATS_SERIES_VALUE="0"
if [[ -n "${pirate_CONTROL_API_HOST_STATS_SERIES:-}" ]]; then
  case "${pirate_CONTROL_API_HOST_STATS_SERIES,,}" in
    1|true|yes|y) CONTROL_API_HOST_STATS_SERIES_VALUE="1" ;;
    0|false|no|n) CONTROL_API_HOST_STATS_SERIES_VALUE="0" ;;
    *)
      echo "Ошибка: pirate_CONTROL_API_HOST_STATS_SERIES — ожидается 0/1, true/false, yes/no, y/n." >&2
      exit 1
      ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  echo "Предупреждение: сохранение истории метрик хостов (CONTROL_API_HOST_STATS_SERIES) и потоковая"
  echo "телеметрия (CONTROL_API_HOST_STATS_STREAM) увеличивают нагрузку на CPU, диск и сеть; на слабом"
  echo "сервере чаще имеет смысл отключить оба варианта (Enter — нет)."
  read -r -p "Включить исторические данные для графиков (CONTROL_API_HOST_STATS_SERIES)? [y/N]: " _host_series
  _host_series="${_host_series#"${_host_series%%[![:space:]]*}"}"
  _host_series="${_host_series%"${_host_series##*[![:space:]]}"}"
  case "${_host_series,,}" in
    y|yes|1|true|д|да) CONTROL_API_HOST_STATS_SERIES_VALUE="1" ;;
    ""|n|no|0|false) CONTROL_API_HOST_STATS_SERIES_VALUE="0" ;;
    *)
      echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
      exit 1
      ;;
  esac
fi

CONTROL_API_HOST_STATS_STREAM_VALUE="0"
if [[ -n "${pirate_CONTROL_API_HOST_STATS_STREAM:-}" ]]; then
  case "${pirate_CONTROL_API_HOST_STATS_STREAM,,}" in
    1|true|yes|y) CONTROL_API_HOST_STATS_STREAM_VALUE="1" ;;
    0|false|no|n) CONTROL_API_HOST_STATS_STREAM_VALUE="0" ;;
    *)
      echo "Ошибка: pirate_CONTROL_API_HOST_STATS_STREAM — ожидается 0/1, true/false, yes/no, y/n." >&2
      exit 1
      ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  if [[ -n "${pirate_CONTROL_API_HOST_STATS_SERIES:-}" ]]; then
    echo "Потоковая телеметрия (STREAM), как и история (SERIES), на слабом сервере может заметно нагружать систему."
  fi
  read -r -p "Включить потоковую телеметрию для онлайн-обновления в UI (CONTROL_API_HOST_STATS_STREAM)? [y/N]: " _host_stream
  _host_stream="${_host_stream#"${_host_stream%%[![:space:]]*}"}"
  _host_stream="${_host_stream%"${_host_stream##*[![:space:]]}"}"
  case "${_host_stream,,}" in
    y|yes|1|true|д|да) CONTROL_API_HOST_STATS_STREAM_VALUE="1" ;;
    ""|n|no|0|false) CONTROL_API_HOST_STATS_STREAM_VALUE="0" ;;
    *)
      echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
      exit 1
      ;;
  esac
fi

# GUI probe (для вопроса и для host-gui-install.json): приоритет bin/pirate gui-check → bin/client gui-check → pirate-gui-probe.sh.
install_gui_probe_json_default='{"gui_detected":false,"reasons":[],"monitor_count":null}'
INSTALL_GUI_PROBE_JSON="$install_gui_probe_json_default"

_validate_install_gui_json_line() {
  [[ -n "${1:-}" ]] || return 1
  if command -v python3 >/dev/null 2>&1; then
    python3 -c "import json,sys; json.loads(sys.argv[1])" "$1" >/dev/null 2>&1
  else
    # До apt-get python3 может отсутствовать — грубая проверка одной строки JSON.
    case "$1" in
      \{*\"gui_detected\"*) return 0 ;;
      *) return 1 ;;
    esac
  fi
}

_try_install_gui_check_bin() {
  local _bin="$1"
  [[ -f "$_bin" ]] || return 1
  [[ -x "$_bin" ]] || chmod +x "$_bin" 2>/dev/null || true
  [[ -x "$_bin" ]] || return 1
  local _line
  _line="$("$_bin" gui-check 2>/dev/null | tail -n 1)" || true
  if _validate_install_gui_json_line "$_line"; then
    INSTALL_GUI_PROBE_JSON="$_line"
    return 0
  fi
  return 1
}

PROBE_SCRIPT="$SCRIPT_DIR/lib/pirate/pirate-gui-probe.sh"
if [[ -f "$BIN_LOCAL/pirate" ]]; then
  _try_install_gui_check_bin "$BIN_LOCAL/pirate" || true
fi
if [[ "$INSTALL_GUI_PROBE_JSON" == "$install_gui_probe_json_default" ]] && [[ -f "$BIN_LOCAL/client" ]]; then
  _try_install_gui_check_bin "$BIN_LOCAL/client" || true
fi
if [[ "$INSTALL_GUI_PROBE_JSON" == "$install_gui_probe_json_default" ]]; then
  if [[ -f "$PROBE_SCRIPT" ]]; then
    _shell_line="$(bash "$PROBE_SCRIPT" 2>/dev/null | tail -n 1)"
    if _validate_install_gui_json_line "$_shell_line"; then
      INSTALL_GUI_PROBE_JSON="$_shell_line"
    fi
  fi
fi

PIRATE_DISPLAY_STREAM_CONSENT_VALUE="0"
if [[ -n "${pirate_DISPLAY_STREAM_CONSENT:-}" ]]; then
  case "${pirate_DISPLAY_STREAM_CONSENT,,}" in
    1|true|yes|y) PIRATE_DISPLAY_STREAM_CONSENT_VALUE="1" ;;
    0|false|no|n) PIRATE_DISPLAY_STREAM_CONSENT_VALUE="0" ;;
    *)
      echo "Ошибка: pirate_DISPLAY_STREAM_CONSENT — ожидается 0/1, true/false, yes/no, y/n." >&2
      exit 1
      ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  GUI_FLAG=0
  if command -v python3 >/dev/null 2>&1; then
    GUI_FLAG="$(python3 -c "import json,sys; print(1 if json.loads(sys.argv[1]).get('gui_detected') else 0)" "$INSTALL_GUI_PROBE_JSON" 2>/dev/null || echo 0)"
  fi
  if [[ "$GUI_FLAG" == "1" ]]; then
    echo ""
    read -r -p "Обнаружен графический рабочий стол. Разрешить клиентам трансляцию экрана (display stream) с этого хоста? [y/N]: " _ds_consent
    _ds_consent="${_ds_consent#"${_ds_consent%%[![:space:]]*}"}"
    _ds_consent="${_ds_consent%"${_ds_consent##*[![:space:]]}"}"
    case "${_ds_consent,,}" in
      y|yes|1|true|д|да) PIRATE_DISPLAY_STREAM_CONSENT_VALUE="1" ;;
      ""|n|no|0|false) PIRATE_DISPLAY_STREAM_CONSENT_VALUE="0" ;;
      *)
        echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
        exit 1
        ;;
    esac
  fi
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
if [[ "$pirate_NGINX" == "1" ]]; then
  echo "==> apt: nginx, openssl"
  apt-get install -y -qq nginx openssl ca-certificates
else
  echo "==> apt: openssl (без nginx)"
  apt-get install -y -qq openssl ca-certificates
fi

# Скрипты из архива: lib/pirate/; из git-клона: каталог рядом с install.sh
if [[ -d "$SCRIPT_DIR/lib/pirate" ]]; then
  PIRATE_LIB_DIR="$SCRIPT_DIR/lib/pirate"
else
  PIRATE_LIB_DIR="$SCRIPT_DIR"
fi

POSTGRES_EXPLORER_LINE=""

if [[ "${pirate_INSTALL_CIFS:-0}" == "1" ]]; then
  echo "==> apt: cifs-utils (SMB через control-api)"
  apt-get install -y -qq cifs-utils
fi

if [[ "${pirate_INSTALL_POSTGRESQL:-0}" == "1" ]]; then
  echo "==> опционально: PostgreSQL (explorer)"
  _pg_tmp="$(mktemp)"
  if [[ -x "$PIRATE_LIB_DIR/install-postgresql.sh" ]]; then
    bash "$PIRATE_LIB_DIR/install-postgresql.sh" 2>&1 | tee "$_pg_tmp"
    POSTGRES_EXPLORER_LINE="$(grep '^POSTGRES_EXPLORER_URL=' "$_pg_tmp" 2>/dev/null | tail -1 || true)"
  else
    echo "Предупреждение: не найден install-postgresql.sh в $PIRATE_LIB_DIR" >&2
  fi
  rm -f "$_pg_tmp"
fi

if [[ "${pirate_INSTALL_MYSQL:-0}" == "1" ]] && [[ -x "$PIRATE_LIB_DIR/install-mysql.sh" ]]; then
  echo "==> опционально: MySQL"
  bash "$PIRATE_LIB_DIR/install-mysql.sh"
fi
if [[ "${pirate_INSTALL_REDIS:-0}" == "1" ]] && [[ -x "$PIRATE_LIB_DIR/install-redis.sh" ]]; then
  echo "==> опционально: Redis"
  bash "$PIRATE_LIB_DIR/install-redis.sh"
fi
if [[ "${pirate_INSTALL_MONGODB:-0}" == "1" ]] && [[ -x "$PIRATE_LIB_DIR/install-mongodb.sh" ]]; then
  echo "==> опционально: MongoDB"
  bash "$PIRATE_LIB_DIR/install-mongodb.sh"
fi
if [[ "${pirate_INSTALL_MSSQL:-0}" == "1" ]] && [[ -x "$PIRATE_LIB_DIR/install-mssql.sh" ]]; then
  echo "==> опционально: MS SQL"
  bash "$PIRATE_LIB_DIR/install-mssql.sh"
fi
if [[ "${pirate_INSTALL_CLICKHOUSE:-0}" == "1" ]] && [[ -x "$PIRATE_LIB_DIR/install-clickhouse.sh" ]]; then
  echo "==> опционально: ClickHouse"
  bash "$PIRATE_LIB_DIR/install-clickhouse.sh"
fi
if [[ "${pirate_INSTALL_ORACLE_NOTES:-0}" == "1" ]] && [[ -x "$PIRATE_LIB_DIR/install-oracle-notes.sh" ]]; then
  echo "==> опционально: Oracle (заметки / скрипт)"
  bash "$PIRATE_LIB_DIR/install-oracle-notes.sh"
fi

echo "==> пользователь и каталоги"
if id deploy &>/dev/null && ! id pirate &>/dev/null; then
  echo "Предупреждение: найден старый пользователь «deploy»; ожидается «pirate». Удалите deploy после миграции или см. purge-pirate-data.sh." >&2
fi
if ! id pirate &>/dev/null; then
  useradd --create-home --home-dir /var/lib/pirate --shell /bin/bash pirate
fi
usermod -aG sudo pirate 2>/dev/null || true

install -d -o pirate -g pirate -m 0755 /var/lib/pirate/deploy
mkdir -p /var/lib/pirate/db-mounts/.creds
install -d -o pirate -g pirate -m 0755 /var/lib/pirate/antiddos/projects
chown -R pirate:pirate /var/lib/pirate/db-mounts
chmod 700 /var/lib/pirate/db-mounts /var/lib/pirate/db-mounts/.creds
if [[ "$pirate_UI" == "1" ]]; then
  install -d -o pirate -g pirate -m 0755 /var/lib/pirate/ui
fi

echo "==> helper scripts -> /usr/local/lib/pirate"
install -d -m 0755 /usr/local/lib/pirate
shopt -s nullglob
for _pf in "$PIRATE_LIB_DIR"/*.sh; do
  _bn="$(basename "$_pf")"
  if [[ "$_bn" == "install.sh" ]] || [[ "$_bn" == "uninstall.sh" ]] || [[ "$_bn" == "purge-pirate-data.sh" ]]; then
    continue
  fi
  install -m 0755 "$_pf" "/usr/local/lib/pirate/$_bn"
done
shopt -u nullglob

echo "==> uninstall scripts -> /usr/local/share/pirate-uninstall"
install -d -m 0755 /usr/local/share/pirate-uninstall
install -m 0755 "$SCRIPT_DIR/uninstall.sh" /usr/local/share/pirate-uninstall/uninstall.sh
install -m 0755 "$SCRIPT_DIR/purge-pirate-data.sh" /usr/local/share/pirate-uninstall/purge-pirate-data.sh
printf '%s\n' "$SCRIPT_DIR" > /var/lib/pirate/original-bundle-path
chown pirate:pirate /var/lib/pirate/original-bundle-path
chmod 0644 /var/lib/pirate/original-bundle-path

echo "==> sudoers (SMB helpers + OTA server-stack apply; только фиксированные пути)"
# NOPASSWD list: single source of truth — 99-pirate-smb.sudoers.fragment (bundled under lib/pirate for OTA).
SUDOERS_PIRATE=/etc/sudoers.d/99-pirate-smb
SUDOERS_FRAG="$SCRIPT_DIR/lib/pirate/99-pirate-smb.sudoers.fragment"
if [[ ! -f "$SUDOERS_FRAG" ]]; then
  echo "Ошибка: не найден $SUDOERS_FRAG" >&2
  exit 1
fi
install -m 0440 "$SUDOERS_FRAG" "$SUDOERS_PIRATE"
if command -v visudo >/dev/null 2>&1; then
  visudo -c -f "$SUDOERS_PIRATE" || {
    echo "Ошибка: проверка sudoers не прошла" >&2
    rm -f "$SUDOERS_PIRATE"
    exit 1
  }
fi

# SMB: control-api вызывает sudo pirate-smb-*.sh; пароли SMB не в SQLite, см. data_sources API.
# /var/lib/pirate/db-mounts — файлы кредов для внешних БД и точки монтирования SMB.

echo "==> бинарники -> /usr/local/bin"
install -m 0755 "$BIN_LOCAL/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_LOCAL/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_LOCAL/client" /usr/local/bin/client
# То же приложение deploy-client, что и pirate (два bin в Cargo.toml). В новых архивах есть оба файла;
# в старых — только client, тогда симлинк (корректный CARGO_BIN_NAME в `pirate --help` даёт отдельный бинарник).
if [[ -f "$BIN_LOCAL/pirate" ]]; then
  install -m 0755 "$BIN_LOCAL/pirate" /usr/local/bin/pirate
else
  ( cd /usr/local/bin && ln -sf client pirate )
fi
if [[ -f "$BIN_LOCAL/pirate-host-agent" ]]; then
  echo "==> pirate-host-agent -> /usr/local/bin (out-of-band OTA / reboot)"
  install -m 0755 "$BIN_LOCAL/pirate-host-agent" /usr/local/bin/pirate-host-agent
fi

if [[ -f "$SCRIPT_DIR/server-stack-manifest.json" ]]; then
  echo "==> server-stack manifest -> /var/lib/pirate (release metadata for GetServerStackInfo)"
  install -m 0644 "$SCRIPT_DIR/server-stack-manifest.json" /var/lib/pirate/server-stack-manifest.json
  chown pirate:pirate /var/lib/pirate/server-stack-manifest.json
  if command -v python3 >/dev/null 2>&1; then
    REL_LINE="$(python3 -c "import json,sys; print((json.load(open(sys.argv[1])).get('release') or '').strip())" "$SCRIPT_DIR/server-stack-manifest.json")"
    if [[ -n "$REL_LINE" ]]; then
      printf '%s\n' "$REL_LINE" > /var/lib/pirate/server-stack-version
      chown pirate:pirate /var/lib/pirate/server-stack-version
      chmod 0644 /var/lib/pirate/server-stack-version
    fi
  fi
fi

if [[ "$pirate_UI" == "1" ]]; then
  echo "==> frontend -> /var/lib/pirate/ui/dist"
  rm -rf /var/lib/pirate/ui/dist
  cp -a "$UI_SRC" /var/lib/pirate/ui/dist
  chown -R pirate:pirate /var/lib/pirate/ui
  # Домашний каталог pirate — /var/lib/pirate; useradd часто ставит 0700/0750.
  # nginx (www-data) должен иметь execute на каждом компоненте пути к root UI.
  if [[ "$pirate_NGINX" == "1" ]]; then
    chmod o+x /var/lib/pirate
  fi
fi

echo "==> SQLite: файл метаданных deploy (история, data_sources, пользователи UI)"
touch /var/lib/pirate/deploy/deploy.db
chown pirate:pirate /var/lib/pirate/deploy/deploy.db
chmod 0640 /var/lib/pirate/deploy/deploy.db

if command -v python3 >/dev/null 2>&1; then
  echo "==> snapshot GUI / display-stream consent -> /var/lib/pirate/host-gui-install.json"
  python3 -c 'import json,time,sys
d=json.loads(sys.argv[1])
d["display_stream_consent"]=int(sys.argv[2])
d["ts_unix"]=int(time.time())
path=sys.argv[3]
with open(path, "w", encoding="utf-8") as f:
    f.write(json.dumps(d))
' "$INSTALL_GUI_PROBE_JSON" "$PIRATE_DISPLAY_STREAM_CONSENT_VALUE" /var/lib/pirate/host-gui-install.json
  chown pirate:pirate /var/lib/pirate/host-gui-install.json
  chmod 0644 /var/lib/pirate/host-gui-install.json
else
  echo "Предупреждение: python3 не найден — host-gui-install.json не записан (установите python3 или задайте pirate_DISPLAY_STREAM_CONSENT вручную)." >&2
fi

echo "==> /etc/pirate-deploy.env"
DEPLOY_SQLITE_URL="sqlite:///var/lib/pirate/deploy/deploy.db"
pirate_PUBLIC_IP=""
if [[ -z "$pirate_DOMAIN" ]]; then
  pirate_PUBLIC_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
fi
umask 077
{
  cat <<EOF
DEPLOY_SQLITE_URL=${DEPLOY_SQLITE_URL}
DEPLOY_ROOT=/var/lib/pirate/deploy
GRPC_ENDPOINT=http://[::1]:50051
CONTROL_API_PORT=8080
RUST_LOG=info
CONTROL_API_BIND=127.0.0.1
DEPLOY_ALLOW_SERVER_STACK_UPDATE=${DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE}
CONTROL_API_HOST_STATS_SERIES=${CONTROL_API_HOST_STATS_SERIES_VALUE}
CONTROL_API_HOST_STATS_STREAM=${CONTROL_API_HOST_STATS_STREAM_VALUE}
PIRATE_DISPLAY_STREAM_CONSENT=${PIRATE_DISPLAY_STREAM_CONSENT_VALUE}
EOF
  if [[ "$pirate_UI" == "1" ]]; then
    cat <<EOF
CONTROL_UI_ADMIN_USERNAME=${UI_ADMIN_NAME}
CONTROL_UI_ADMIN_PASSWORD=${UI_ADMIN_PASS}
CONTROL_API_JWT_SECRET=${CONTROL_API_JWT_SECRET_VALUE}
EOF
  fi
  if [[ -n "$POSTGRES_EXPLORER_LINE" ]]; then
    echo "$POSTGRES_EXPLORER_LINE"
  fi
  if [[ -n "$pirate_DOMAIN" ]]; then
    echo "DEPLOY_GRPC_PUBLIC_URL=http://${pirate_DOMAIN}:50051"
  else
    echo "DEPLOY_GRPC_PUBLIC_URL=http://${pirate_PUBLIC_IP:-127.0.0.1}:50051"
  fi
} >/etc/pirate-deploy.env
chmod 0640 /etc/pirate-deploy.env
chown root:pirate /etc/pirate-deploy.env

echo "==> systemd"
install -m 0644 "$SYSTEMD_SRC/deploy-server.service" /etc/systemd/system/deploy-server.service
install -m 0644 "$SYSTEMD_SRC/control-api.service" /etc/systemd/system/control-api.service
if [[ -f /usr/local/bin/pirate-host-agent ]] && [[ -f "$SYSTEMD_SRC/pirate-host-agent.service" ]]; then
  install -m 0644 "$SYSTEMD_SRC/pirate-host-agent.service" /etc/systemd/system/pirate-host-agent.service
  if [[ ! -f /etc/pirate-host-agent.env ]]; then
    if command -v openssl >/dev/null 2>&1; then
      _HA_TOKEN="$(openssl rand -hex 32)"
    elif command -v python3 >/dev/null 2>&1; then
      _HA_TOKEN="$(python3 -c "import secrets; print(secrets.token_hex(32))")"
    else
      echo "Ошибка: для токена pirate-host-agent нужен openssl или python3." >&2
      exit 1
    fi
    umask 077
    {
      echo "PIRATE_HOST_AGENT_TOKEN=${_HA_TOKEN}"
      echo "PIRATE_HOST_AGENT_BIND=127.0.0.1:9443"
    } >/etc/pirate-host-agent.env
    chmod 0600 /etc/pirate-host-agent.env
    echo "Создан /etc/pirate-host-agent.env — сохраните токен для Pirate Client (out-of-band агент на 127.0.0.1:9443)." >&2
  fi
fi
systemctl daemon-reload
systemctl enable deploy-server.service control-api.service
if [[ -f /etc/systemd/system/pirate-host-agent.service ]]; then
  systemctl enable pirate-host-agent.service
fi

if [[ "$pirate_NGINX" == "1" ]]; then
  echo "==> nginx"
  if [[ "$pirate_UI" == "1" ]]; then
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
  else
    if [[ -n "$pirate_DOMAIN" ]]; then
      if [[ ! -f "$NGINX_SRC/nginx-pirate-api-only-domain.conf.in" ]]; then
        echo "Не найдено: $NGINX_SRC/nginx-pirate-api-only-domain.conf.in" >&2
        exit 1
      fi
      if [[ "$pirate_DOMAIN" == www.* ]]; then
        SERVER_NAMES="$pirate_DOMAIN"
      else
        SERVER_NAMES="$pirate_DOMAIN www.$pirate_DOMAIN"
      fi
      sed "s|__pirate_SERVER_NAMES__|${SERVER_NAMES}|g" \
        "$NGINX_SRC/nginx-pirate-api-only-domain.conf.in" >/etc/nginx/sites-available/pirate
      chmod 0644 /etc/nginx/sites-available/pirate
    else
      if [[ ! -f "$NGINX_SRC/nginx-pirate-api-only.conf" ]]; then
        echo "Не найдено: $NGINX_SRC/nginx-pirate-api-only.conf" >&2
        exit 1
      fi
      install -m 0644 "$NGINX_SRC/nginx-pirate-api-only.conf" /etc/nginx/sites-available/pirate
    fi
  fi
  if [[ -f /etc/nginx/sites-enabled/default ]]; then
    rm -f /etc/nginx/sites-enabled/default
  fi
  ln -sf /etc/nginx/sites-available/pirate /etc/nginx/sites-enabled/pirate
  nginx -t
  systemctl enable nginx
  systemctl restart nginx
fi

echo "==> запуск сервисов"
systemctl restart deploy-server.service
sleep 1

echo "==> gRPC: ключ control-api → deploy-server (reconcile / дашборд)"
sudo -u pirate env DEPLOY_ROOT=/var/lib/pirate/deploy /usr/local/bin/control-api bootstrap-grpc-key

GRPC_KEY_LINE='GRPC_SIGNING_KEY_PATH=/var/lib/pirate/deploy/.keys/control_api_ed25519.json'
if grep -q '^GRPC_SIGNING_KEY_PATH=' /etc/pirate-deploy.env 2>/dev/null; then
  sed -i "s|^GRPC_SIGNING_KEY_PATH=.*|${GRPC_KEY_LINE}|" /etc/pirate-deploy.env
else
  echo "$GRPC_KEY_LINE" >> /etc/pirate-deploy.env
fi
chmod 0640 /etc/pirate-deploy.env
chown root:pirate /etc/pirate-deploy.env

echo "==> перезапуск deploy-server (подхват authorized_peers после bootstrap)"
systemctl restart deploy-server.service
sleep 1
systemctl restart control-api.service

if [[ -f /etc/systemd/system/pirate-host-agent.service ]]; then
  echo "==> pirate-host-agent"
  systemctl restart pirate-host-agent.service || true
fi

echo ""
echo "Готово."
echo "  Метаданные: SQLite (${DEPLOY_SQLITE_URL})"
if [[ "$pirate_UI" == "1" ]]; then
  echo "  Дашборд (control-api): логин ${UI_ADMIN_NAME}, пароль (также в /etc/pirate-deploy.env): $UI_ADMIN_PASS"
else
  echo "  control-api: без JWT (HTTP API на 127.0.0.1:8080 без авторизации; только с этого хоста)."
  if [[ -f "$NO_UI_MARKER" ]]; then
    echo "             Веб-дашборд в этом архиве не включён (UI_BUILD=0); для JWT/UI нужен бандл с UI_BUILD=1."
  else
    echo "             Добавить дашборд и JWT: sudo $0 --ui  (с nginx: sudo $0 --nginx --ui). Pair gRPC не меняется."
  fi
fi
if [[ "$pirate_NGINX" == "1" ]] && [[ "$pirate_UI" == "1" ]]; then
  if [[ -n "$pirate_DOMAIN" ]]; then
    echo "  UI:        http://${pirate_DOMAIN}/"
  else
    _ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
    echo "  UI:        http://${_ip}:80/"
    echo "             Домен не задан — доступ по IP и порту 80 (HTTP)."
  fi
elif [[ "$pirate_NGINX" == "1" ]] && [[ "$pirate_UI" != "1" ]]; then
  if [[ -n "$pirate_DOMAIN" ]]; then
    echo "  HTTP API за nginx: http://${pirate_DOMAIN}/api/ … (статика UI не установлена, см. --ui)"
  else
    _ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
    echo "  HTTP API за nginx: http://${_ip}/api/ … (статика UI не установлена, см. --ui)"
  fi
elif [[ "$pirate_UI" == "1" ]] && [[ "$pirate_NGINX" != "1" ]]; then
  echo "  Статика UI: /var/lib/pirate/ui/dist — nginx не настраивался; подключите веб-сервер или переустановите с --nginx."
fi
if [[ -n "$pirate_DOMAIN" ]]; then
  echo "  gRPC URL для pairing (также в /etc/pirate-deploy.env): http://${pirate_DOMAIN}:50051"
  echo "             Откройте порт 50051 в firewall, если клиенты подключаются не с этого хоста."
else
  _ip2="$(hostname -I 2>/dev/null | awk '{print $1}')"
  echo "  gRPC URL для pairing (также в /etc/pirate-deploy.env): http://${_ip2:-127.0.0.1}:50051"
  echo "             Откройте порт 50051 в firewall, если клиенты подключаются не с этого хоста."
fi
echo "  API health: curl -s http://127.0.0.1:8080/health"
echo "  Клиент (CLI: client и pirate — одно приложение, pirate → client в /usr/local/bin):"
echo "           JSON для pair (поля token, url, pairing):"
sudo -u pirate bash -c 'set -a; . /etc/pirate-deploy.env; set +a; exec /usr/local/bin/deploy-server --root /var/lib/pirate/deploy print-install-bundle'
echo "           Пример: pirate pair --bundle '<JSON выше>'   или: client pair --bundle ./bundle.json"
echo "           Версии: pirate --version   или   pirate --version-all [--endpoint URL]"
echo "           Без pair команды pirate status / deploy вернут missing metadata (x-deploy-pubkey)."
echo "           После pair: pirate status / deploy (URL сохраняется из поля url в JSON)."
echo "  Проверка gRPC: не через curl к :50051 (это HTTP/2 gRPC); используйте pirate (или client) или grpcurl."
echo ""
echo "Логи: journalctl -u deploy-server -f   /   journalctl -u control-api -f"
