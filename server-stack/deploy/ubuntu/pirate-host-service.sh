#!/usr/bin/env bash
# Whitelist dispatcher for control-api: install/remove optional host packages;
# show-runtime/apply-runtime/restart for pirate-managed MinIO/Meilisearch env files.
# Usage (as root): pirate-host-service.sh <action> <id>   (apply-runtime: env file on stdin)
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
  minio) bash "$DIR/install-minio.sh" ;;
  meilisearch) bash "$DIR/install-meilisearch.sh" ;;
  stack_tun_api)
    [[ -f /usr/local/bin/stack-tun-api ]] || die "stack-tun-api binary missing (/usr/local/bin/stack-tun-api)"
    [[ -f /etc/systemd/system/pirate-stack-tun-api.service ]] || die "pirate-stack-tun-api.service missing"
    mkdir -p /var/lib/pirate/stack-tun-api
    chown pirate:pirate /var/lib/pirate/stack-tun-api
    chmod 0750 /var/lib/pirate/stack-tun-api
    bash "$DIR/pirate-ensure-stack-tun-env.sh"
    systemctl daemon-reload
    systemctl enable --now pirate-stack-tun-api.service
    ;;
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
  minio) bash "$DIR/remove-minio.sh" ;;
  meilisearch) bash "$DIR/remove-meilisearch.sh" ;;
  stack_tun_api)
    systemctl disable --now pirate-stack-tun-api.service 2>/dev/null || true
    ;;
  cifs_utils) bash "$DIR/remove-cifs-utils.sh" ;;
  *) die "unknown id: $ID" ;;
  esac
  ;;
show-runtime)
  case "$ID" in
  minio)
    if [[ -f /etc/pirate-minio.env ]]; then cat /etc/pirate-minio.env; fi
    ;;
  meilisearch)
    if [[ -f /etc/pirate-meilisearch.env ]]; then cat /etc/pirate-meilisearch.env; fi
    ;;
  *) die "show-runtime not supported for: $ID" ;;
  esac
  ;;
apply-runtime)
  case "$ID" in
  minio)
    TMP="$(mktemp)" || die "mktemp failed"
    cat >"$TMP"
    install -m 0600 -o root -g root "$TMP" /etc/pirate-minio.env
    rm -f "$TMP"
    systemctl restart pirate-minio
    echo "pirate-minio: runtime config applied and service restarted"
    ;;
  meilisearch)
    TMP="$(mktemp)" || die "mktemp failed"
    cat >"$TMP"
    install -m 0600 -o root -g root "$TMP" /etc/pirate-meilisearch.env
    rm -f "$TMP"
    systemctl restart pirate-meilisearch
    echo "pirate-meilisearch: runtime config applied and service restarted"
    ;;
  *) die "apply-runtime not supported for: $ID" ;;
  esac
  ;;
restart)
  case "$ID" in
  minio) systemctl restart pirate-minio ;;
  meilisearch) systemctl restart pirate-meilisearch ;;
  *) die "restart not supported for: $ID" ;;
  esac
  echo "restarted: $ID"
  ;;
*) die "usage: $0 install|remove|show-runtime|apply-runtime|restart <id>" ;;
esac
