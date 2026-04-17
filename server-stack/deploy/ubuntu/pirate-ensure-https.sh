#!/usr/bin/env bash
set -euo pipefail

DOMAIN="${1:-}"
EMAIL="${2:-}"
if [[ -z "$DOMAIN" ]]; then
  echo "usage: pirate-ensure-https.sh <domain> [email]" >&2
  exit 2
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y certbot python3-certbot-nginx

if [[ -n "$EMAIL" ]]; then
  certbot --nginx -d "$DOMAIN" --agree-tos --no-eff-email --email "$EMAIL" --non-interactive
else
  certbot --nginx -d "$DOMAIN" --agree-tos --register-unsafely-without-email --non-interactive
fi

systemctl enable --now certbot.timer >/dev/null 2>&1 || true
echo "OK: https ensured for ${DOMAIN}"
