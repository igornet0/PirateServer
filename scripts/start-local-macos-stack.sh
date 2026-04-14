#!/usr/bin/env bash
# Локальный стек на macOS (из репозитория): deploy-server + control-api + те же вопросы, что в dist install.sh.
# Не требует sudo (в отличие от установки в /var/lib/pirate).
#
# Запуск (из корня репозитория):
#   make -f Makefile.test build-ui
#   make -f Makefile.test start-local-server-macos
#   (по умолчанию PORT=50052 BIND=127.0.0.1; на macOS — проверка дисплея через pirate gui-check; SKIP_GUI_CHECK=1 — без проверки)
#
# Без вопросов (как pirate_NONINTERACTIVE=1 в install.sh):
#   pirate_NONINTERACTIVE=1 \
#   pirate_DOMAIN= \
#   pirate_UI_ADMIN_USERNAME=admin \
#   pirate_UI_ADMIN_PASSWORD=secret \
#   pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE=0 \
#   PORT=50052 BIND=127.0.0.1 make -f Makefile.test start-local-server-macos
#
# Минимально только gRPC без дашборда (старый режим):
#   LOCAL_STACK_MINIMAL=1 PORT=50052 make -f Makefile.test start-local-server-macos
#
# Данные по умолчанию — эфемерный каталог repo/tmp/pirate-local-stack.XXXXXX (создаётся перед
# запуском, после остановки скрипта удаляется). Постоянное хранилище: DEPLOY_LOCAL_ROOT=/path/to/dir
#
# Дашборд в браузере: второй терминал — Vite проксирует /api на control-api:
#   cd server-stack/frontend && npm run dev
#   → http://localhost:5173

set -euo pipefail

if [[ "${EUID:-0}" -eq 0 ]]; then
  echo "Запускайте без sudo: deploy-server отказывается работать от root." >&2
  exit 1
fi

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

CARGO="${CARGO:-cargo}"
PORT="${PORT:-50052}"
BIND="${BIND:-127.0.0.1}"
CONTROL_API_PORT="${CONTROL_API_PORT:-8080}"

# Пустой DEPLOY_LOCAL_ROOT → эфемерный каталог в repo/tmp/ (удаляется в trap при выходе).
# Задан явно → постоянный root (не удаляем).
EPHEMERAL_DEPLOY_ROOT=0
_explicit_root="${DEPLOY_LOCAL_ROOT:-}"
if [[ -z "$_explicit_root" ]]; then
  mkdir -p "$REPO_ROOT/tmp"
  DEPLOY_LOCAL_ROOT="$(mktemp -d "$REPO_ROOT/tmp/pirate-local-stack.XXXXXX")"
  EPHEMERAL_DEPLOY_ROOT=1
else
  mkdir -p "$_explicit_root"
  DEPLOY_LOCAL_ROOT="$(cd "$_explicit_root" && pwd)"
fi
unset _explicit_root

if [[ "${LOCAL_STACK_MINIMAL:-0}" == "1" ]]; then
  cleanup_minimal() {
    if [[ "$EPHEMERAL_DEPLOY_ROOT" == "1" ]] && [[ -n "${DEPLOY_LOCAL_ROOT:-}" ]]; then
      rm -rf "$DEPLOY_LOCAL_ROOT"
    fi
  }
  trap cleanup_minimal EXIT INT TERM
  echo "==> deploy-server (LOCAL_STACK_MINIMAL=1, DEPLOY_GRPC_ALLOW_UNAUTHENTICATED)"
  echo "    root: $DEPLOY_LOCAL_ROOT"
  echo "    listen: ${BIND}:${PORT}"
  if [[ "$EPHEMERAL_DEPLOY_ROOT" == "1" ]]; then
    echo "    (эфемерный каталог, после выхода будет удалён)"
  fi
  env \
    DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1 \
    DEPLOY_GRPC_PUBLIC_URL="http://${BIND}:${PORT}" \
    RUST_LOG="${RUST_LOG:-info}" \
    "$CARGO" run -p deploy-server -- \
    --root "$DEPLOY_LOCAL_ROOT" \
    --bind "$BIND" \
    -p "$PORT"
  exit 0
fi

# Полный стек: на macOS проверяем наличие GUI / мониторов (как `pirate gui-check` для display-stream).
if [[ "$(uname -s)" == "Darwin" ]] && [[ "${SKIP_GUI_CHECK:-}" != "1" ]]; then
  echo "==> проверка GUI / дисплея (pirate gui-check)"
  _gui_json="$("$CARGO" run -p deploy-client --bin pirate -- gui-check 2>/dev/null | tail -n 1)"
  echo "$_gui_json"
  if command -v python3 >/dev/null 2>&1 && [[ -n "$_gui_json" ]]; then
    if python3 -c "import json,sys; sys.exit(0 if json.loads(sys.argv[1]).get('gui_detected') else 1)" "$_gui_json" 2>/dev/null; then
      :
    else
      echo "Предупреждение: gui_detected=false — для display-stream нужен дисплей и разрешение «Запись экрана» (Системные настройки → Конфиденциальность)." >&2
    fi
  fi
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "Нужен openssl (Xcode Command Line Tools)." >&2
  exit 1
fi

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

primary_lan_ip() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    local iface
    iface="$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')"
    if [[ -n "$iface" ]]; then
      ipconfig getifaddr "$iface" 2>/dev/null || true
    fi
  else
    hostname -I 2>/dev/null | awk '{print $1}'
  fi
}

# --- Вопросы как в dist install.sh (с --ui): домен, админ, пароль, OTA, host stats ---
pirate_DOMAIN="${pirate_DOMAIN:-}"
pirate_DOMAIN="${pirate_DOMAIN#"${pirate_DOMAIN%%[![:space:]]*}"}"
pirate_DOMAIN="${pirate_DOMAIN%"${pirate_DOMAIN##*[![:space:]]}"}"

if [[ -z "$pirate_DOMAIN" ]] && [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  read -r -p "Домен для веб-интерфейса (Enter — без домена, UI через Vite: см. конец вывода): " pirate_DOMAIN || true
  pirate_DOMAIN="${pirate_DOMAIN#"${pirate_DOMAIN%%[![:space:]]*}"}"
  pirate_DOMAIN="${pirate_DOMAIN%"${pirate_DOMAIN##*[![:space:]]}"}"
fi

if [[ -n "$pirate_DOMAIN" ]]; then
  validate_domain "$pirate_DOMAIN" || {
    echo "Ошибка: недопустимое имя хоста: $pirate_DOMAIN" >&2
    exit 1
  }
fi

_dash_name_def="admin"
_dash_pass_rand="$(openssl rand -base64 24 | tr -d '/+=' | head -c 24)"
UI_ADMIN_NAME=""
UI_ADMIN_PASS=""

if [[ -n "${pirate_UI_ADMIN_USERNAME:-}" ]]; then
  UI_ADMIN_NAME="${pirate_UI_ADMIN_USERNAME#"${pirate_UI_ADMIN_USERNAME%%[![:space:]]*}"}"
  UI_ADMIN_NAME="${UI_ADMIN_NAME%"${UI_ADMIN_NAME##*[![:space:]]}"}"
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  read -r -p "Имя пользователя веб-дашборда [${_dash_name_def}]: " _in_dash_name || true
  _in_dash_name="${_in_dash_name#"${_in_dash_name%%[![:space:]]*}"}"
  _in_dash_name="${_in_dash_name%"${_in_dash_name##*[![:space:]]}"}"
  UI_ADMIN_NAME="${_in_dash_name:-$_dash_name_def}"
else
  UI_ADMIN_NAME="$_dash_name_def"
fi
[[ -z "$UI_ADMIN_NAME" ]] && UI_ADMIN_NAME="$_dash_name_def"
if [[ ! "$UI_ADMIN_NAME" =~ ^[A-Za-z0-9._-]+$ ]] || [[ ${#UI_ADMIN_NAME} -gt 64 ]]; then
  echo "Ошибка: имя пользователя дашборда — латиница, цифры, . _ - (1–64)." >&2
  exit 1
fi

if [[ -n "${pirate_UI_ADMIN_PASSWORD:-}" ]]; then
  UI_ADMIN_PASS="$pirate_UI_ADMIN_PASSWORD"
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  read -r -s -p "Пароль веб-дашборда (Enter — случайный): " _in_dash_pass || true
  echo ""
  if [[ -z "${_in_dash_pass:-}" ]]; then
    UI_ADMIN_PASS="$_dash_pass_rand"
  else
    UI_ADMIN_PASS="$_in_dash_pass"
  fi
else
  UI_ADMIN_PASS="$_dash_pass_rand"
fi

CONTROL_API_JWT_SECRET_VALUE="$(openssl rand -base64 48 | tr -d '\n')"

DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0"
if [[ -n "${pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE:-}" ]]; then
  case "${pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE,,}" in
    1 | true | yes | y) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="1" ;;
    0 | false | no | n) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0" ;;
    *)
      echo "Ошибка: pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE — 0/1." >&2
      exit 1
      ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  read -r -p "Разрешить OTA-обновление server-stack через gRPC (DEPLOY_ALLOW_SERVER_STACK_UPDATE)? [y/N]: " _allow_stack || true
  _allow_stack="${_allow_stack#"${_allow_stack%%[![:space:]]*}"}"
  _allow_stack="${_allow_stack%"${_allow_stack##*[![:space:]]}"}"
  case "${_allow_stack,,}" in
    y | yes | 1 | true | д | да) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="1" ;;
    "" | n | no | 0 | false) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0" ;;
    *)
      echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
      exit 1
      ;;
  esac
fi

CONTROL_API_HOST_STATS_SERIES_VALUE="0"
if [[ -n "${pirate_CONTROL_API_HOST_STATS_SERIES:-}" ]]; then
  case "${pirate_CONTROL_API_HOST_STATS_SERIES,,}" in
    1 | true | yes | y) CONTROL_API_HOST_STATS_SERIES_VALUE="1" ;;
    *) CONTROL_API_HOST_STATS_SERIES_VALUE="0" ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  echo ""
  echo "Предупреждение: сохранение истории метрик хостов (CONTROL_API_HOST_STATS_SERIES) и потоковая"
  echo "телеметрия (CONTROL_API_HOST_STATS_STREAM) увеличивают нагрузку на CPU, диск и сеть; на слабом"
  echo "сервере чаще имеет смысл отключить оба варианта (Enter — нет)."
  read -r -p "Включить исторические данные для графиков (CONTROL_API_HOST_STATS_SERIES)? [y/N]: " _host_series || true
  _host_series="${_host_series#"${_host_series%%[![:space:]]*}"}"
  _host_series="${_host_series%"${_host_series##*[![:space:]]}"}"
  case "${_host_series,,}" in
    y | yes | 1 | true | д | да) CONTROL_API_HOST_STATS_SERIES_VALUE="1" ;;
    "" | n | no | 0 | false) CONTROL_API_HOST_STATS_SERIES_VALUE="0" ;;
    *)
      echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
      exit 1
      ;;
  esac
fi

CONTROL_API_HOST_STATS_STREAM_VALUE="0"
if [[ -n "${pirate_CONTROL_API_HOST_STATS_STREAM:-}" ]]; then
  case "${pirate_CONTROL_API_HOST_STATS_STREAM,,}" in
    1 | true | yes | y) CONTROL_API_HOST_STATS_STREAM_VALUE="1" ;;
    *) CONTROL_API_HOST_STATS_STREAM_VALUE="0" ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  if [[ -n "${pirate_CONTROL_API_HOST_STATS_SERIES:-}" ]]; then
    echo "Потоковая телеметрия (STREAM), как и история (SERIES), на слабом сервере может заметно нагружать систему."
  fi
  read -r -p "Включить потоковую телеметрию для онлайн-обновления в UI (CONTROL_API_HOST_STATS_STREAM)? [y/N]: " _host_stream || true
  _host_stream="${_host_stream#"${_host_stream%%[![:space:]]*}"}"
  _host_stream="${_host_stream%"${_host_stream##*[![:space:]]}"}"
  case "${_host_stream,,}" in
    y | yes | 1 | true | д | да) CONTROL_API_HOST_STATS_STREAM_VALUE="1" ;;
    "" | n | no | 0 | false) CONTROL_API_HOST_STATS_STREAM_VALUE="0" ;;
    *)
      echo "Ошибка: введите y/да или n; пустой ввод — нет." >&2
      exit 1
      ;;
  esac
fi

mkdir -p "$DEPLOY_LOCAL_ROOT/db-mounts"
chmod u+rwx "$DEPLOY_LOCAL_ROOT" "$DEPLOY_LOCAL_ROOT/db-mounts" 2>/dev/null || true
# sqlx 0.8: по умолчанию create_if_missing=false — без существующего файла SQLite даёт code 14.
# В dist install.sh перед стартом делают `touch …/deploy.db`; эквивалент — ?mode=rwc в URI.
DEPLOY_SQLITE_URL="sqlite://${DEPLOY_LOCAL_ROOT}/deploy.db?mode=rwc"

pirate_PUBLIC_IP=""
if [[ -z "$pirate_DOMAIN" ]]; then
  pirate_PUBLIC_IP="$(primary_lan_ip)"
fi

if [[ -n "$pirate_DOMAIN" ]]; then
  DEPLOY_GRPC_PUBLIC_URL="http://${pirate_DOMAIN}:${PORT}"
else
  DEPLOY_GRPC_PUBLIC_URL="http://${pirate_PUBLIC_IP:-127.0.0.1}:${PORT}"
fi

# gRPC для control-api: тот же хост/порт, что слушает deploy-server
if [[ "$BIND" == "::" ]]; then
  GRPC_ENDPOINT="http://[::1]:${PORT}"
else
  GRPC_ENDPOINT="http://127.0.0.1:${PORT}"
fi

export DEPLOY_ROOT="$DEPLOY_LOCAL_ROOT"

SERVER_PID=""
API_PID=""

cleanup() {
  if [[ -n "${API_PID:-}" ]] && kill -0 "$API_PID" 2>/dev/null; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [[ "$EPHEMERAL_DEPLOY_ROOT" == "1" ]] && [[ -n "${DEPLOY_LOCAL_ROOT:-}" ]]; then
    rm -rf "$DEPLOY_LOCAL_ROOT"
  fi
}
trap cleanup EXIT INT TERM

echo ""
echo "==> Локальный стек (как после install.sh в dist, без nginx/systemd)"
echo "    DEPLOY_ROOT:     $DEPLOY_LOCAL_ROOT"
if [[ "$EPHEMERAL_DEPLOY_ROOT" == "1" ]]; then
  echo "    (эфемерный каталог — после остановки будет удалён)"
fi
echo "    gRPC listen:     ${BIND}:${PORT}"
echo "    gRPC public URL: $DEPLOY_GRPC_PUBLIC_URL"
echo "    control-api:     127.0.0.1:${CONTROL_API_PORT}"
echo ""

# deploy-server читает authorized_peers.json только при старте; после bootstrap-grpc-key
# нужен перезапуск процесса (как launchctl kickstart deploy-server в install.sh).
start_deploy_server_bg() {
  RUST_LOG="${RUST_LOG:-info}" \
    DEPLOY_SQLITE_URL="$DEPLOY_SQLITE_URL" \
    DEPLOY_GRPC_PUBLIC_URL="$DEPLOY_GRPC_PUBLIC_URL" \
    DEPLOY_ALLOW_SERVER_STACK_UPDATE="$DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE" \
    CONTROL_UI_ADMIN_USERNAME="$UI_ADMIN_NAME" \
    CONTROL_UI_ADMIN_PASSWORD="$UI_ADMIN_PASS" \
    "$CARGO" run -p deploy-server -- \
    --root "$DEPLOY_LOCAL_ROOT" \
    --bind "$BIND" \
    -p "$PORT" &
  SERVER_PID=$!
}

echo "==> Запуск deploy-server…"
start_deploy_server_bg

echo "==> Ожидание готовности (.keys)…"
for _i in $(seq 1 40); do
  if [[ -d "$DEPLOY_LOCAL_ROOT/.keys" ]]; then
    break
  fi
  sleep 0.25
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "deploy-server завершился до готовности." >&2
    wait "$SERVER_PID" || true
    exit 1
  fi
done

if [[ ! -d "$DEPLOY_LOCAL_ROOT/.keys" ]]; then
  echo "Таймаут: нет каталога .keys — проверьте логи deploy-server." >&2
  exit 1
fi

echo "==> bootstrap-grpc-key (как после install.sh на сервере)"
RUST_LOG="${RUST_LOG:-warn}" \
  DEPLOY_ROOT="$DEPLOY_LOCAL_ROOT" \
  "$CARGO" run -p control-api -- bootstrap-grpc-key

echo "==> Перезапуск deploy-server (подхват ключа control-api в authorized_peers)"
if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
  kill "$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
fi
start_deploy_server_bg
sleep 1
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "deploy-server не поднялся после перезапуска." >&2
  exit 1
fi

GRPC_KEY_PATH="$DEPLOY_LOCAL_ROOT/.keys/control_api_ed25519.json"

echo "==> Запуск control-api (Ctrl+C — остановить оба процесса)…"
RUST_LOG="${RUST_LOG:-info}" \
  DEPLOY_SQLITE_URL="$DEPLOY_SQLITE_URL" \
  GRPC_ENDPOINT="$GRPC_ENDPOINT" \
  GRPC_SIGNING_KEY_PATH="$GRPC_KEY_PATH" \
  CONTROL_API_BIND=127.0.0.1 \
  CONTROL_API_PORT="$CONTROL_API_PORT" \
  CONTROL_API_JWT_SECRET="$CONTROL_API_JWT_SECRET_VALUE" \
  CONTROL_API_HOST_STATS_SERIES="$CONTROL_API_HOST_STATS_SERIES_VALUE" \
  CONTROL_API_HOST_STATS_STREAM="$CONTROL_API_HOST_STATS_STREAM_VALUE" \
  CONTROL_API_CORS_ALLOW_ANY=1 \
  PIRATE_DATA_MOUNTS_ROOT="$DEPLOY_LOCAL_ROOT/db-mounts" \
  "$CARGO" run -p control-api -- \
  --deploy-root "$DEPLOY_LOCAL_ROOT" \
  --listen-port "$CONTROL_API_PORT" \
  --bind 127.0.0.1 &
API_PID=$!

sleep 1
if [[ "$UI_ADMIN_PASS" == "$_dash_pass_rand" ]] && [[ -z "${pirate_UI_ADMIN_PASSWORD:-}" ]]; then
  echo ""
  echo "Сгенерирован пароль веб-дашборда (сохраните): $UI_ADMIN_PASS"
fi
echo ""
echo "Клиент gRPC (pair): пока этот процесс работает, в другом терминале:"
echo "  cd \"$REPO_ROOT\" && $CARGO run -p deploy-server -- --root \"$DEPLOY_LOCAL_ROOT\" --bind \"$BIND\" -p $PORT print-install-bundle"
echo ""
echo "API:  curl -s http://127.0.0.1:${CONTROL_API_PORT}/health"
echo "UI:   cd server-stack/frontend && npm run dev  →  http://localhost:5173  (прокси /api на control-api)"
echo "Статика уже собрана: $REPO_ROOT/server-stack/frontend/dist (для nginx/отдачи как на сервере)."
echo ""
echo "Остановка: Ctrl+C"

wait "$API_PID" || true
