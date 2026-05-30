# Build from PirateServer workspace root (--context ../../../.. relative to compose file).
# Keep Rust image reasonably current — some transitive crates require edition2024 (Cargo parses their manifests).
FROM rust:bookworm AS builder
WORKDIR /src
COPY . .
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && cargo build --release -p stack-tun-api -p deploy-client \
        --bin stack-tun-api --bin pirate \
    && strip /src/target/release/stack-tun-api /src/target/release/pirate || true

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
       ca-certificates curl openssl jq python3 libxcb1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/stack-tun-api /usr/local/bin/stack-tun-api
COPY --from=builder /src/target/release/pirate /usr/local/bin/pirate
COPY server-stack/stack-tun-api/docker/integration/scripts /opt/scripts
RUN chmod +x /opt/scripts/*.sh
ENV STACK_TUN_REST_BEARER=e2e-bearer-token
EXPOSE 9380 9381
WORKDIR /
ENTRYPOINT ["/bin/bash"]
