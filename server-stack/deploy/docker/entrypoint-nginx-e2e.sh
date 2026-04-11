#!/usr/bin/env bash
set -euo pipefail
# Starts a minimal nginx master (for nginx -t / reload in control-api), then control-api.
# Intended for docker-compose.nginx-e2e.yml only.

if [[ -d /data ]]; then
  chown -R deploy:deploy /data
fi

# nginx worker needs writable dirs when master runs as non-root (gosu deploy)
mkdir -p /var/lib/nginx/body /var/lib/nginx/proxy /var/lib/nginx/fastcgi \
  /var/lib/nginx/uwsgi /var/lib/nginx/scgi /var/cache/nginx/client_temp
chown -R deploy:deploy /var/lib/nginx /var/cache/nginx

CONF=/data/nginx-e2e.conf
cat >"$CONF" <<'EOF'
worker_processes 1;
pid /tmp/nginx-e2e.pid;
error_log /tmp/nginx-e2e-error.log;
events { worker_connections 128; }
http {
  access_log /dev/null;
  server {
    listen 18089;
    return 204 '';
  }
}
EOF
chown deploy:deploy "$CONF"

gosu deploy nginx -c "$CONF"
sleep 0.3

exec gosu deploy "$@"
