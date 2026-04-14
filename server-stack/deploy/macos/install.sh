#!/usr/bin/env bash
# Установка на macOS (x86_64 или arm64; бинарники Mach-O из бандла).
#
#   sudo ./install.sh                      — launchd + бинарники, без nginx и без UI
#   sudo ./install.sh --nginx              — + nginx (Homebrew): прокси /api
#   sudo ./install.sh --ui                 — + share/ui/dist
#   sudo ./install.sh --nginx --ui         — полный веб-стек
#
# Каталог: pirate-macos-amd64/ или pirate-macos-arm64/ (bin/, share/, launchd/, install.sh).
# Опциональные СУБД (pirate_INSTALL_* из Ubuntu) на macOS не поддерживаются — см. README.

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
NO_UI_MARKER="$SCRIPT_DIR/.bundle-no-ui"

usage() {
  echo "Использование: sudo $0 [--domain FQDN] [--nginx] [--ui]" >&2
  echo "  Полный веб-стек: sudo $0 --nginx --ui" >&2
  if [[ -f "$NO_UI_MARKER" ]]; then
    echo "  Архив без UI (.bundle-no-ui): --ui недопустим." >&2
  fi
  echo "  С --nginx нужен Homebrew и пакет nginx (скрипт выполнит brew install при необходимости)." >&2
  echo "  Без вопросов: sudo pirate_NONINTERACTIVE=1 $0" >&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --nginx) pirate_NGINX=1; shift ;;
    --ui)
      if [[ -f "$NO_UI_MARKER" ]]; then
        echo "Ошибка: архив без статики дашборда (UI_BUILD=0)." >&2
        exit 1
      fi
      pirate_UI=1
      shift
      ;;
    --domain)
      [[ -n "${2:-}" ]] || { echo "Ошибка: --domain требует значение." >&2; exit 1; }
      pirate_DOMAIN="$2"
      shift 2
      ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Неизвестный аргумент: $1" >&2; usage; exit 1 ;;
  esac
done

pirate_NGINX="${pirate_NGINX:-0}"
pirate_UI="${pirate_UI:-0}"
if [[ -f "$NO_UI_MARKER" ]] && [[ "$pirate_UI" == "1" ]]; then
  echo "Ошибка: pirate_UI=1 недопустим при .bundle-no-ui." >&2
  exit 1
fi

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

if [[ "$pirate_UI" == "1" ]] && [[ -z "$pirate_DOMAIN" ]] && [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  read -r -p "Домен для веб-интерфейса (Enter — без домена, по IP): " pirate_DOMAIN || true
  pirate_DOMAIN="${pirate_DOMAIN#"${pirate_DOMAIN%%[![:space:]]*}"}"
  pirate_DOMAIN="${pirate_DOMAIN%"${pirate_DOMAIN##*[![:space:]]}"}"
fi

if [[ -n "$pirate_DOMAIN" ]]; then
  validate_domain "$pirate_DOMAIN" || { echo "Ошибка: недопустимое имя хоста: $pirate_DOMAIN" >&2; exit 1; }
fi

cd "$SCRIPT_DIR"

BIN_LOCAL="$SCRIPT_DIR/bin"
UI_SRC="$SCRIPT_DIR/share/ui/dist"
LAUNCHD_SRC="$SCRIPT_DIR/launchd"
if [[ -d "$SCRIPT_DIR/nginx" ]]; then
  NGINX_SRC="$SCRIPT_DIR/nginx"
else
  NGINX_SRC="$SCRIPT_DIR"
fi

for p in "$BIN_LOCAL/deploy-server" "$BIN_LOCAL/control-api" "$BIN_LOCAL/client"; do
  if [[ ! -e "$p" ]]; then
    echo "Не найдено: $p — запускайте из распакованного архива pirate-macos-*." >&2
    exit 1
  fi
done
if [[ "$pirate_UI" == "1" ]] && [[ ! -e "$UI_SRC/index.html" ]]; then
  echo "Не найдено: $UI_SRC/index.html — для --ui нужен бандл со статикой UI." >&2
  exit 1
fi

HOST_ARCH="$(uname -m)"
BIN_ARCH="$(file -b "$BIN_LOCAL/deploy-server" 2>/dev/null || true)"
if [[ "$HOST_ARCH" == "arm64" ]] && [[ "$BIN_ARCH" == *"x86_64"* ]]; then
  echo "Ошибка: бинарники x86_64, а хост arm64." >&2
  exit 1
fi
if [[ "$HOST_ARCH" == "x86_64" ]] && [[ "$BIN_ARCH" == *"arm64"* ]]; then
  echo "Ошибка: бинарники arm64, а хост x86_64." >&2
  exit 1
fi

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
fi

DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0"
if [[ -n "${pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE:-}" ]]; then
  case "${pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE,,}" in
    1|true|yes|y) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="1" ;;
    0|false|no|n) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0" ;;
    *) echo "Ошибка: pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE — 0/1." >&2; exit 1 ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  read -r -p "Разрешить OTA server-stack через gRPC? [y/N]: " _allow_stack || true
  _allow_stack="${_allow_stack#"${_allow_stack%%[![:space:]]*}"}"
  case "${_allow_stack,,}" in
    y|yes|1|true|д|да) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="1" ;;
    *) DEPLOY_ALLOW_SERVER_STACK_UPDATE_VALUE="0" ;;
  esac
fi

CONTROL_API_HOST_STATS_SERIES_VALUE="0"
if [[ -n "${pirate_CONTROL_API_HOST_STATS_SERIES:-}" ]]; then
  case "${pirate_CONTROL_API_HOST_STATS_SERIES,,}" in
    1|true|yes|y) CONTROL_API_HOST_STATS_SERIES_VALUE="1" ;;
    *) CONTROL_API_HOST_STATS_SERIES_VALUE="0" ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  read -r -p "CONTROL_API_HOST_STATS_SERIES? [y/N]: " _host_series || true
  case "${_host_series,,}" in y|yes|1|true|д|да) CONTROL_API_HOST_STATS_SERIES_VALUE="1" ;; *) CONTROL_API_HOST_STATS_SERIES_VALUE="0" ;; esac
fi

CONTROL_API_HOST_STATS_STREAM_VALUE="0"
if [[ -n "${pirate_CONTROL_API_HOST_STATS_STREAM:-}" ]]; then
  case "${pirate_CONTROL_API_HOST_STATS_STREAM,,}" in
    1|true|yes|y) CONTROL_API_HOST_STATS_STREAM_VALUE="1" ;;
    *) CONTROL_API_HOST_STATS_STREAM_VALUE="0" ;;
  esac
elif [[ -t 0 ]] && [[ "${pirate_NONINTERACTIVE:-}" != "1" ]]; then
  read -r -p "CONTROL_API_HOST_STATS_STREAM? [y/N]: " _host_stream || true
  case "${_host_stream,,}" in y|yes|1|true|д|да) CONTROL_API_HOST_STATS_STREAM_VALUE="1" ;; *) CONTROL_API_HOST_STATS_STREAM_VALUE="0" ;; esac
fi

if [[ "${pirate_INSTALL_POSTGRESQL:-0}" == "1" ]] || [[ "${pirate_INSTALL_MYSQL:-0}" == "1" ]]; then
  echo "Предупреждение: pirate_INSTALL_* для СУБД на macOS не поддерживаются (см. README)." >&2
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "Ошибка: нужен openssl (установите Xcode Command Line Tools)." >&2
  exit 1
fi

if [[ "$pirate_NGINX" == "1" ]]; then
  if ! command -v brew >/dev/null 2>&1 && [[ ! -x /opt/homebrew/bin/brew ]] && [[ ! -x /usr/local/bin/brew ]]; then
    echo "Ошибка: --nginx требует Homebrew (https://brew.sh)." >&2
    exit 1
  fi
  BREW="$(command -v brew || true)"
  [[ -x "$BREW" ]] || BREW="/opt/homebrew/bin/brew"
  [[ -x "$BREW" ]] || BREW="/usr/local/bin/brew"
  echo "==> brew: nginx, openssl"
  "$BREW" install nginx openssl
fi

ensure_pirate_user() {
  if id pirate &>/dev/null; then
    return 0
  fi
  dseditgroup -o create pirate 2>/dev/null || true
  local maxid newid pgid
  pgid="$(dscl . -read /Groups/pirate PrimaryGroupID 2>/dev/null | awk '{print $2}')"
  if [[ -z "$pgid" ]]; then
    echo "Ошибка: не удалось создать группу pirate (dseditgroup)." >&2
    exit 1
  fi
  maxid=$(dscl . -list /Users UniqueID 2>/dev/null | awk '{print $2}' | sort -n | tail -1)
  newid=$((maxid + 1))
  dscl . -create /Users/pirate
  dscl . -create /Users/pirate UserShell /bin/bash
  dscl . -create /Users/pirate UniqueID "$newid"
  dscl . -create /Users/pirate PrimaryGroupID "$pgid"
  dscl . -create /Users/pirate NFSHomeDirectory /var/lib/pirate
  dseditgroup -o edit -a pirate -t user pirate 2>/dev/null || true
}

echo "==> пользователь и каталоги"
ensure_pirate_user
install -d -o pirate -g pirate -m 0755 /var/lib/pirate/deploy
mkdir -p /var/lib/pirate/db-mounts/.creds
chown -R pirate:pirate /var/lib/pirate/db-mounts
chmod 700 /var/lib/pirate/db-mounts /var/lib/pirate/db-mounts/.creds
if [[ "$pirate_UI" == "1" ]]; then
  install -d -o pirate -g pirate -m 0755 /var/lib/pirate/ui
fi

if [[ -d "$SCRIPT_DIR/lib/pirate" ]]; then
  PIRATE_LIB_DIR="$SCRIPT_DIR/lib/pirate"
else
  PIRATE_LIB_DIR="$SCRIPT_DIR"
fi

echo "==> helper scripts -> /usr/local/lib/pirate"
install -d -m 0755 /usr/local/lib/pirate
shopt -s nullglob
for _pf in "$PIRATE_LIB_DIR"/*.sh; do
  _bn="$(basename "$_pf")"
  if [[ "$_bn" == "install.sh" ]] || [[ "$_bn" == "uninstall.sh" ]] || [[ "$_bn" == "purge-pirate-data.sh" ]]; then
    continue
  fi
  if [[ "$_bn" == run-deploy-server.sh ]] || [[ "$_bn" == run-control-api.sh ]]; then
    continue
  fi
  install -m 0755 "$_pf" "/usr/local/lib/pirate/$_bn"
done
shopt -u nullglob

echo "==> libexec (launchd) -> /usr/local/libexec/pirate"
install -d -m 0755 /usr/local/libexec/pirate
for w in run-deploy-server.sh run-control-api.sh; do
  if [[ -f "$PIRATE_LIB_DIR/$w" ]]; then
    install -m 0755 "$PIRATE_LIB_DIR/$w" "/usr/local/libexec/pirate/$w"
  fi
done

echo "==> uninstall scripts -> /usr/local/share/pirate-uninstall"
install -d -m 0755 /usr/local/share/pirate-uninstall
install -m 0755 "$SCRIPT_DIR/uninstall.sh" /usr/local/share/pirate-uninstall/uninstall.sh
install -m 0755 "$SCRIPT_DIR/purge-pirate-data.sh" /usr/local/share/pirate-uninstall/purge-pirate-data.sh
printf '%s\n' "$SCRIPT_DIR" > /var/lib/pirate/original-bundle-path
chown pirate:pirate /var/lib/pirate/original-bundle-path
chmod 0644 /var/lib/pirate/original-bundle-path

echo "==> sudoers"
SUDOERS_PIRATE=/etc/sudoers.d/99-pirate-smb
cat >"$SUDOERS_PIRATE" <<'SUDOERS'
# Pirate: non-interactive sudo for SMB helpers and stack OTA helper
pirate ALL=(root) NOPASSWD: /usr/local/lib/pirate/pirate-smb-mount.sh, /usr/local/lib/pirate/pirate-smb-umount.sh, /usr/local/lib/pirate/pirate-apply-stack-bundle.sh
SUDOERS
chmod 0440 "$SUDOERS_PIRATE"
if command -v visudo >/dev/null 2>&1; then
  visudo -c -f "$SUDOERS_PIRATE" || {
    echo "Ошибка: visudo" >&2
    rm -f "$SUDOERS_PIRATE"
    exit 1
  }
fi

echo "==> бинарники -> /usr/local/bin"
install -m 0755 "$BIN_LOCAL/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_LOCAL/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_LOCAL/client" /usr/local/bin/client
if [[ -f "$BIN_LOCAL/pirate" ]]; then
  install -m 0755 "$BIN_LOCAL/pirate" /usr/local/bin/pirate
else
  ( cd /usr/local/bin && ln -sf client pirate )
fi

if [[ -f "$SCRIPT_DIR/server-stack-manifest.json" ]]; then
  echo "==> server-stack manifest"
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
  if [[ "$pirate_NGINX" == "1" ]]; then
    chmod o+x /var/lib/pirate 2>/dev/null || true
  fi
fi

touch /var/lib/pirate/deploy/deploy.db
chown pirate:pirate /var/lib/pirate/deploy/deploy.db
chmod 0640 /var/lib/pirate/deploy/deploy.db

mac_primary_ip() {
  local iface
  iface="$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')"
  if [[ -n "$iface" ]]; then
    ipconfig getifaddr "$iface" 2>/dev/null || true
  fi
}

echo "==> /etc/pirate-deploy.env"
DEPLOY_SQLITE_URL="sqlite:///var/lib/pirate/deploy/deploy.db"
pirate_PUBLIC_IP=""
if [[ -z "$pirate_DOMAIN" ]]; then
  pirate_PUBLIC_IP="$(mac_primary_ip)"
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
EOF
  if [[ "$pirate_UI" == "1" ]]; then
    cat <<EOF
CONTROL_UI_ADMIN_USERNAME=${UI_ADMIN_NAME}
CONTROL_UI_ADMIN_PASSWORD=${UI_ADMIN_PASS}
CONTROL_API_JWT_SECRET=${CONTROL_API_JWT_SECRET_VALUE}
EOF
  fi
  if [[ -n "$pirate_DOMAIN" ]]; then
    echo "DEPLOY_GRPC_PUBLIC_URL=http://${pirate_DOMAIN}:50051"
  else
    echo "DEPLOY_GRPC_PUBLIC_URL=http://${pirate_PUBLIC_IP:-127.0.0.1}:50051"
  fi
} >/etc/pirate-deploy.env
chmod 0640 /etc/pirate-deploy.env
chown root:pirate /etc/pirate-deploy.env

echo "==> launchd"
install -d -m 0755 /var/log/pirate
for pl in com.pirate.deploy-server.plist com.pirate.control-api.plist; do
  if [[ ! -f "$LAUNCHD_SRC/$pl" ]]; then
    echo "Не найдено: $LAUNCHD_SRC/$pl" >&2
    exit 1
  fi
  install -m 0644 "$LAUNCHD_SRC/$pl" "/Library/LaunchDaemons/$pl"
done
launchctl bootout system /Library/LaunchDaemons/com.pirate.deploy-server.plist 2>/dev/null || true
launchctl bootout system /Library/LaunchDaemons/com.pirate.control-api.plist 2>/dev/null || true
launchctl bootstrap system /Library/LaunchDaemons/com.pirate.deploy-server.plist
launchctl bootstrap system /Library/LaunchDaemons/com.pirate.control-api.plist

if [[ "$pirate_NGINX" == "1" ]]; then
  echo "==> nginx (Homebrew)"
  BREW="$(command -v brew 2>/dev/null || true)"
  [[ -x "$BREW" ]] || BREW="/opt/homebrew/bin/brew"
  [[ -x "$BREW" ]] || BREW="/usr/local/bin/brew"
  NGINX_PREFIX="$("$BREW" --prefix nginx)"
  SERVERS_DIR="$NGINX_PREFIX/etc/nginx/servers"
  install -d -m 0755 "$SERVERS_DIR"
  if [[ "$pirate_UI" == "1" ]]; then
    if [[ -n "$pirate_DOMAIN" ]]; then
      if [[ "$pirate_DOMAIN" == www.* ]]; then
        SERVER_NAMES="$pirate_DOMAIN"
      else
        SERVER_NAMES="$pirate_DOMAIN www.$pirate_DOMAIN"
      fi
      sed "s|__pirate_SERVER_NAMES__|${SERVER_NAMES}|g" \
        "$NGINX_SRC/nginx-pirate-site-domain.conf.in" >"$SERVERS_DIR/pirate.conf"
    else
      install -m 0644 "$NGINX_SRC/nginx-pirate-site.conf" "$SERVERS_DIR/pirate.conf"
    fi
  else
    if [[ -n "$pirate_DOMAIN" ]]; then
      if [[ "$pirate_DOMAIN" == www.* ]]; then
        SERVER_NAMES="$pirate_DOMAIN"
      else
        SERVER_NAMES="$pirate_DOMAIN www.$pirate_DOMAIN"
      fi
      sed "s|__pirate_SERVER_NAMES__|${SERVER_NAMES}|g" \
        "$NGINX_SRC/nginx-pirate-api-only-domain.conf.in" >"$SERVERS_DIR/pirate.conf"
    else
      install -m 0644 "$NGINX_SRC/nginx-pirate-api-only.conf" "$SERVERS_DIR/pirate.conf"
    fi
  fi
  if [[ -f "$NGINX_PREFIX/etc/nginx/nginx.conf" ]] && ! grep -q 'include servers/\*' "$NGINX_PREFIX/etc/nginx/nginx.conf" 2>/dev/null; then
    echo "Внимание: добавьте в $NGINX_PREFIX/etc/nginx/nginx.conf в блок http: include servers/*;" >&2
  fi
  "$NGINX_PREFIX/bin/nginx" -t
  "$BREW" services restart nginx || "$NGINX_PREFIX/bin/nginx" -s reload || true
fi

echo "==> запуск сервисов"
launchctl kickstart -k "system/com.pirate.deploy-server" 2>/dev/null || true
sleep 1

echo "==> gRPC: bootstrap-grpc-key"
sudo -u pirate env DEPLOY_ROOT=/var/lib/pirate/deploy /usr/local/bin/control-api bootstrap-grpc-key

GRPC_KEY_LINE='GRPC_SIGNING_KEY_PATH=/var/lib/pirate/deploy/.keys/control_api_ed25519.json'
export GRPC_KEY_LINE
python3 <<'PYE'
import os, re
p = "/etc/pirate-deploy.env"
line = os.environ["GRPC_KEY_LINE"]
with open(p, "r", encoding="utf-8") as f:
    s = f.read()
if re.search(r"^GRPC_SIGNING_KEY_PATH=", s, re.M):
    s = re.sub(r"^GRPC_SIGNING_KEY_PATH=.*$", line, s, flags=re.M)
else:
    s = s.rstrip() + ("\n" if s and not s.endswith("\n") else "") + line + "\n"
with open(p, "w", encoding="utf-8") as f:
    f.write(s)
PYE
chmod 0640 /etc/pirate-deploy.env
chown root:pirate /etc/pirate-deploy.env

launchctl kickstart -k "system/com.pirate.deploy-server" 2>/dev/null || true
sleep 1
launchctl kickstart -k "system/com.pirate.control-api" 2>/dev/null || true

echo ""
echo "Готово (macOS)."
echo "  Логи: log stream --style syslog --predicate 'subsystem == \"com.apple.launchd\"' ... или /var/log/pirate/"
echo "  API health: curl -s http://127.0.0.1:8080/health"
if [[ "$pirate_NGINX" == "1" ]] && [[ "$pirate_UI" == "1" ]]; then
  if [[ -n "$pirate_DOMAIN" ]]; then
    echo "  UI: http://${pirate_DOMAIN}/"
  else
    _ip="$(mac_primary_ip)"
    echo "  UI: http://${_ip:-127.0.0.1}:80/ (если nginx слушает :80)"
  fi
fi
echo "  Клиент: sudo -u pirate ... deploy-server print-install-bundle — см. Makefile в каталоге бандла."
