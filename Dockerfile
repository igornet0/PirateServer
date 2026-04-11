# Multi-target: Rust binaries + static UI + nginx (for docker-compose.test.yml).
# (No `# syntax=docker/dockerfile:1` here — avoids an extra pull from registry-1.docker.io
# before build; use it only if you need a specific Dockerfile frontend version.)

FROM rust:bookworm AS rust-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY proto proto
COPY server-stack/server server-stack/server
COPY server-stack/control-api server-stack/control-api
COPY server-stack/deploy-auth server-stack/deploy-auth
COPY server-stack/deploy-control server-stack/deploy-control
COPY server-stack/deploy-db server-stack/deploy-db
COPY server-stack/deploy-core server-stack/deploy-core
COPY server-stack/tunnel-gateway server-stack/tunnel-gateway
COPY local-stack local-stack
RUN cargo build --release -p deploy-server -p control-api -p deploy-client

FROM debian:bookworm-slim AS runtime-base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 curl gosu jq \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 1000 -d /deploy -s /bin/bash deploy
COPY server-stack/deploy/docker/entrypoint-deploy.sh /usr/local/bin/entrypoint-deploy.sh
RUN chmod +x /usr/local/bin/entrypoint-deploy.sh
COPY --from=rust-builder /app/target/release/deploy-server /usr/local/bin/deploy-server
COPY --from=rust-builder /app/target/release/control-api /usr/local/bin/control-api
COPY --from=rust-builder /app/target/release/client /usr/local/bin/client
WORKDIR /deploy
ENTRYPOINT ["/usr/local/bin/entrypoint-deploy.sh"]

# control-api + nginx in one container for nginx /api/v1/nginx/config e2e (see docker-compose.nginx-e2e.yml).
FROM runtime-base AS runtime-nginx-e2e
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends nginx \
    && rm -rf /var/lib/apt/lists/*
COPY server-stack/deploy/docker/entrypoint-nginx-e2e.sh /usr/local/bin/entrypoint-nginx-e2e.sh
RUN chmod +x /usr/local/bin/entrypoint-nginx-e2e.sh
ENTRYPOINT ["/usr/local/bin/entrypoint-nginx-e2e.sh"]

FROM node:20-bookworm AS frontend-build
WORKDIR /ui
COPY server-stack/frontend/package.json server-stack/frontend/package-lock.json ./
RUN npm ci
COPY server-stack/frontend ./
RUN npm run build

FROM nginx:1.27-alpine AS nginx-with-ui
COPY --from=frontend-build /ui/dist /usr/share/nginx/html
COPY server-stack/deploy/docker/nginx-docker.conf /etc/nginx/conf.d/default.conf
