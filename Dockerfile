# Multi-target: Rust binaries + static UI + nginx (for tests/docker/docker-compose.test.yml).
# (No `# syntax=docker/dockerfile:1` here — avoids an extra pull from registry-1.docker.io
# before build; use it only if you need a specific Dockerfile frontend version.)

FROM rust:bookworm AS rust-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY proto proto
COPY wire-protocol wire-protocol
COPY server-stack/server server-stack/server
COPY server-stack/control-api server-stack/control-api
COPY server-stack/deploy-auth server-stack/deploy-auth
COPY server-stack/deploy-control server-stack/deploy-control
COPY server-stack/deploy-db server-stack/deploy-db
COPY server-stack/deploy-core server-stack/deploy-core
COPY server-stack/ingress-config server-stack/ingress-config
COPY server-stack/tunnel-gateway server-stack/tunnel-gateway
COPY server-stack/xray-export server-stack/xray-export
COPY server-stack/desktop-ui server-stack/desktop-ui
COPY local-stack local-stack
RUN cargo build --release -p deploy-server -p control-api -p deploy-client

FROM debian:bookworm-slim AS runtime-base
# deploy-client links xcap (GUI probe) → libxcb at runtime; slim image omits X11 libs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 curl gosu jq libxcb1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 1000 -d /deploy -s /bin/bash deploy
COPY server-stack/deploy/docker/entrypoint-deploy.sh /usr/local/bin/entrypoint-deploy.sh
RUN chmod +x /usr/local/bin/entrypoint-deploy.sh
COPY --from=rust-builder /app/target/release/deploy-server /usr/local/bin/deploy-server
COPY --from=rust-builder /app/target/release/control-api /usr/local/bin/control-api
COPY --from=rust-builder /app/target/release/client /usr/local/bin/client
WORKDIR /deploy
ENTRYPOINT ["/usr/local/bin/entrypoint-deploy.sh"]

# Same as runtime-base plus grpcurl and grpc-security-probe (protocol-bench / extended security checks).
FROM runtime-base AS runtime-bench
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends wget ca-certificates iproute2 iputils-ping \
    && GRPCURL_VER=1.9.1 \
    && case "${TARGETARCH:-amd64}" in \
         amd64) GARCH=x86_64 ;; \
         arm64) GARCH=arm64 ;; \
         *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2 && exit 1 ;; \
       esac \
    && wget -q "https://github.com/fullstorydev/grpcurl/releases/download/v${GRPCURL_VER}/grpcurl_${GRPCURL_VER}_linux_${GARCH}.tar.gz" -O /tmp/g.tgz \
    && tar -xzf /tmp/g.tgz -C /usr/local/bin grpcurl \
    && chmod +x /usr/local/bin/grpcurl \
    && rm -f /tmp/g.tgz \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust-builder /app/target/release/grpc-security-probe /usr/local/bin/grpc-security-probe

# control-api + nginx in one container for nginx /api/v1/nginx/config e2e (see tests/docker/docker-compose.nginx-e2e.yml).
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
