-- Peer connection profile, resource telemetry, proxy traffic buckets, server benchmark scores.

CREATE TABLE grpc_peer_profile (
    client_pubkey_b64 TEXT PRIMARY KEY,
    connection_kind SMALLINT NOT NULL DEFAULT 0,
    agent_version TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE grpc_peer_resource_snapshot (
    client_pubkey_b64 TEXT PRIMARY KEY,
    cpu_percent DOUBLE PRECISION,
    ram_percent DOUBLE PRECISION,
    gpu_percent DOUBLE PRECISION,
    ram_used_bytes BIGINT,
    storage_used_bytes BIGINT,
    reported_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE grpc_proxy_traffic_hourly (
    client_pubkey_b64 TEXT NOT NULL,
    hour_start TIMESTAMPTZ NOT NULL,
    bytes_in BIGINT NOT NULL DEFAULT 0,
    bytes_out BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (client_pubkey_b64, hour_start)
);

CREATE INDEX idx_grpc_proxy_traffic_hour ON grpc_proxy_traffic_hourly (hour_start DESC);

CREATE TABLE server_resource_benchmark (
    id BIGSERIAL PRIMARY KEY,
    run_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    cpu_score INT NOT NULL,
    ram_score INT NOT NULL,
    storage_score INT NOT NULL,
    gpu_score INT,
    raw_json TEXT NOT NULL DEFAULT ''
);
