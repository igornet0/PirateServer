#!/usr/bin/env bash
# Install PostgreSQL for optional dashboard schema explorer (POSTGRES_EXPLORER_URL).
# Does not replace application metadata (SQLite / DATABASE_URL). Run as root.
#
# Env (optional):
#   PIRATE_POSTGRESQL_LISTEN_ADDRESSES (default 127.0.0.1; use * for all interfaces)
#   PIRATE_POSTGRESQL_PORT (default 5432)
#   PIRATE_EXPLORER_DB_USER / PIRATE_EXPLORER_DB_NAME (defaults pirate_explorer)
#   PIRATE_EXPLORER_DB_PASSWORD (optional; hex generated if empty)
#   PIRATE_EXPLORER_DB_HOST / PIRATE_EXPLORER_DB_PORT (for URL + pg_hba; defaults 127.0.0.1:5432)
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get install -y -qq postgresql

LISTEN="${PIRATE_POSTGRESQL_LISTEN_ADDRESSES:-127.0.0.1}"
PG_PORT="${PIRATE_POSTGRESQL_PORT:-5432}"
DB_USER="${PIRATE_EXPLORER_DB_USER:-pirate_explorer}"
DB_NAME="${PIRATE_EXPLORER_DB_NAME:-pirate_explorer}"
EXPL_HOST="${PIRATE_EXPLORER_DB_HOST:-127.0.0.1}"
EXPL_PORT="${PIRATE_EXPLORER_DB_PORT:-5432}"

case "$PG_PORT" in
'' | *[!0-9]*)
  echo "postgresql: PIRATE_POSTGRESQL_PORT must be a number" >&2
  exit 1
  ;;
esac
if ((PG_PORT < 1 || PG_PORT > 65535)); then
  echo "postgresql: invalid PIRATE_POSTGRESQL_PORT" >&2
  exit 1
fi

case "$EXPL_PORT" in
'' | *[!0-9]*)
  echo "postgresql: PIRATE_EXPLORER_DB_PORT must be a number" >&2
  exit 1
  ;;
esac
if ((EXPL_PORT < 1 || EXPL_PORT > 65535)); then
  echo "postgresql: invalid PIRATE_EXPLORER_DB_PORT" >&2
  exit 1
fi

if [[ ! "$DB_USER" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]]; then
  echo "postgresql: PIRATE_EXPLORER_DB_USER must match ^[a-zA-Z_][a-zA-Z0-9_]*$" >&2
  exit 1
fi
if [[ ! "$DB_NAME" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]]; then
  echo "postgresql: PIRATE_EXPLORER_DB_NAME must match ^[a-zA-Z_][a-zA-Z0-9_]*$" >&2
  exit 1
fi

if [[ "$LISTEN" == *$'\n'* ]] || [[ "$LISTEN" == *$'\r'* ]]; then
  echo "postgresql: PIRATE_POSTGRESQL_LISTEN_ADDRESSES must be a single line" >&2
  exit 1
fi

RESOLVED_EXPL="$EXPL_HOST"
if [[ "$RESOLVED_EXPL" == "localhost" ]]; then
  RESOLVED_EXPL="127.0.0.1"
fi

HBA_NET=""
if [[ "$RESOLVED_EXPL" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]]; then
  HBA_NET="${RESOLVED_EXPL}/32"
elif [[ "$RESOLVED_EXPL" == *:* ]]; then
  HBA_NET="${RESOLVED_EXPL}/128"
else
  echo "postgresql: PIRATE_EXPLORER_DB_HOST must be an IPv4/IPv6 literal or localhost" >&2
  exit 1
fi

PG_PASS="${PIRATE_EXPLORER_DB_PASSWORD:-}"
if [[ -z "$PG_PASS" ]]; then
  if command -v openssl &>/dev/null; then
    PG_PASS="$(openssl rand -hex 16)"
  else
    PG_PASS="$(tr -dc 'a-f0-9' </dev/urandom | head -c 32)"
  fi
fi

cd /
if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='${DB_USER}'" | grep -q 1; then
  sudo -u postgres psql -c "ALTER USER \"${DB_USER}\" WITH PASSWORD '${PG_PASS//\'/\'\'}';" >/dev/null
else
  sudo -u postgres psql -c "CREATE USER \"${DB_USER}\" WITH PASSWORD '${PG_PASS//\'/\'\'}';" >/dev/null
fi

if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='${DB_NAME}'" | grep -q 1; then
  sudo -u postgres psql -c "ALTER DATABASE \"${DB_NAME}\" OWNER TO \"${DB_USER}\";" >/dev/null || true
else
  sudo -u postgres psql -c "CREATE DATABASE \"${DB_NAME}\" OWNER \"${DB_USER}\";" >/dev/null
fi

PG_VER="$(ls /etc/postgresql 2>/dev/null | sort -V | tail -1 || true)"
if [[ -z "$PG_VER" ]]; then
  echo "postgresql: could not detect /etc/postgresql version dir" >&2
  exit 1
fi

PG_CONF="/etc/postgresql/${PG_VER}/main/postgresql.conf"
PG_HBA="/etc/postgresql/${PG_VER}/main/pg_hba.conf"
MARK_BEGIN="# BEGIN pirate-stack postgresql (host-services)"
MARK_END="# END pirate-stack postgresql (host-services)"

if [[ -f "$PG_CONF" ]]; then
  if grep -qF "$MARK_BEGIN" "$PG_CONF" 2>/dev/null; then
    sed -i "/^${MARK_BEGIN//\//\\/}\$/,/^${MARK_END//\//\\/}\$/d" "$PG_CONF"
  fi
  {
    echo ""
    echo "$MARK_BEGIN"
    echo "listen_addresses = '${LISTEN//\'/\'\'}'"
    echo "port = ${PG_PORT}"
    echo "$MARK_END"
  } >>"$PG_CONF"
fi

if [[ -f "$PG_HBA" ]]; then
  if grep -qF "$MARK_BEGIN" "$PG_HBA" 2>/dev/null; then
    sed -i "/^${MARK_BEGIN//\//\\/}\$/,/^${MARK_END//\//\\/}\$/d" "$PG_HBA"
  fi
  LINE="host ${DB_NAME} ${DB_USER} ${HBA_NET} scram-sha-256"
  {
    echo ""
    echo "$MARK_BEGIN"
    echo "$LINE"
    echo "$MARK_END"
  } >>"$PG_HBA"
fi

systemctl enable postgresql
systemctl restart postgresql

URL_USER="$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$DB_USER")"
URL_PASS="$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$PG_PASS")"
URL="postgresql://${URL_USER}:${URL_PASS}@${RESOLVED_EXPL}:${EXPL_PORT}/${DB_NAME}"

echo ""
echo "Add to /etc/pirate-deploy.env (then: systemctl restart control-api):"
echo "POSTGRES_EXPLORER_URL=${URL}"
echo ""
echo "Cluster: listen_addresses=${LISTEN} port=${PG_PORT}"
echo "If POSTGRES_EXPLORER_URL is already set, replace it only if you intend to use this database for the explorer."
