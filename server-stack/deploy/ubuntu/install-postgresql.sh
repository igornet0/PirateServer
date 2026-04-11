#!/usr/bin/env bash
# Install PostgreSQL for optional dashboard schema explorer (POSTGRES_EXPLORER_URL).
# Does not replace application metadata (SQLite / DATABASE_URL). Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get install -y -qq postgresql

DB_USER="pirate_explorer"
DB_NAME="pirate_explorer"
# Hex-only password is safe in postgresql:// URLs without encoding.
PG_PASS="${PIRATE_EXPLORER_DB_PASSWORD:-$(openssl rand -hex 16)}"

cd /
if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='${DB_USER}'" | grep -q 1; then
  sudo -u postgres psql -c "ALTER USER ${DB_USER} WITH PASSWORD '${PG_PASS}';" >/dev/null
else
  sudo -u postgres psql -c "CREATE USER ${DB_USER} WITH PASSWORD '${PG_PASS}';" >/dev/null
fi

if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='${DB_NAME}'" | grep -q 1; then
  :
else
  sudo -u postgres psql -c "CREATE DATABASE ${DB_NAME} OWNER ${DB_USER};" >/dev/null
fi

PG_VER="$(ls /etc/postgresql 2>/dev/null | sort -V | tail -1 || true)"
if [[ -n "$PG_VER" ]]; then
  PG_HBA="/etc/postgresql/${PG_VER}/main/pg_hba.conf"
  if [[ -f "$PG_HBA" ]]; then
    LINE="host ${DB_NAME} ${DB_USER} 127.0.0.1/32 scram-sha-256"
    if ! grep -qF "$LINE" "$PG_HBA"; then
      echo "$LINE" >>"$PG_HBA"
      systemctl reload postgresql
    fi
  fi
fi

systemctl enable postgresql
systemctl restart postgresql

URL="postgresql://${DB_USER}:${PG_PASS}@127.0.0.1:5432/${DB_NAME}"
echo ""
echo "Add to /etc/pirate-deploy.env (then: systemctl restart control-api):"
echo "POSTGRES_EXPLORER_URL=${URL}"
echo ""
echo "If POSTGRES_EXPLORER_URL is already set, replace it only if you intend to use this database for the explorer."
