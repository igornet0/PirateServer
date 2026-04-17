#!/usr/bin/env bash
# Whitelist dispatcher for control-api: install/remove optional host packages.
# Usage (as root): pirate-host-service.sh install|remove <id>
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACTION="${1:-}"
ID="${2:-}"

die() {
  echo "pirate-host-service: $*" >&2
  exit 1
}

[[ "${EUID:-0}" -eq 0 ]] || die "must run as root"

case "$ACTION" in
install)
  case "$ID" in
  node) bash "$DIR/install-nodejs-runtime.sh" ;;
  python3) bash "$DIR/install-python3-runtime.sh" ;;
  nginx)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq nginx
    ;;
  redis) bash "$DIR/install-redis.sh" ;;
  postgresql) bash "$DIR/install-postgresql.sh" ;;
  mysql) bash "$DIR/install-mysql.sh" ;;
  mongodb) bash "$DIR/install-mongodb.sh" ;;
  mssql) bash "$DIR/install-mssql.sh" ;;
  clickhouse) bash "$DIR/install-clickhouse.sh" ;;
  cifs_utils)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq cifs-utils
    ;;
  *) die "unknown id: $ID" ;;
  esac
  ;;
remove)
  case "$ID" in
  node) bash "$DIR/remove-nodejs-runtime.sh" ;;
  python3) bash "$DIR/remove-python3-runtime.sh" ;;
  nginx) bash "$DIR/remove-nginx.sh" ;;
  redis) bash "$DIR/remove-redis.sh" ;;
  postgresql) bash "$DIR/remove-postgresql.sh" ;;
  mysql) bash "$DIR/remove-mysql.sh" ;;
  mongodb) bash "$DIR/remove-mongodb.sh" ;;
  mssql) bash "$DIR/remove-mssql.sh" ;;
  clickhouse) bash "$DIR/remove-clickhouse.sh" ;;
  cifs_utils) bash "$DIR/remove-cifs-utils.sh" ;;
  *) die "unknown id: $ID" ;;
  esac
  ;;
*) die "usage: $0 install|remove <id>" ;;
esac
