#!/usr/bin/env bash
# Ensure CONTROL_API_JWT_SECRET exists in pirate-deploy.env (append if missing/empty).
# Usage: sudo pirate-ensure-jwt-secret.sh [/etc/pirate-deploy.env]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=pirate-env-common.sh
source "$HERE/pirate-env-common.sh"

ENVF="$(pirate_env_normalize_path "${1:-}")"
if [[ ! -f "$ENVF" ]]; then
  echo "Файл не найден: $ENVF" >&2
  exit 1
fi

cur="$(pirate_env_get_raw "$ENVF" CONTROL_API_JWT_SECRET 2>/dev/null || true)"
if [[ -n "${cur// }" ]]; then
  echo "CONTROL_API_JWT_SECRET уже задан в $ENVF"
  exit 0
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "Нужен openssl для генерации CONTROL_API_JWT_SECRET" >&2
  exit 1
fi

jwt="$(openssl rand -base64 48 | tr -d '\n')"
pirate_env_upsert "$ENVF" CONTROL_API_JWT_SECRET "$jwt"
echo "Добавлен CONTROL_API_JWT_SECRET в $ENVF"
