-- Peer connection profile, resource telemetry, proxy traffic buckets, server benchmark scores.

CREATE TABLE grpc_peer_profile (
    client_pubkey_b64 TEXT PRIMARY KEY,
    connection_kind INTEGER NOT NULL DEFAULT 0,
    agent_version TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE grpc_peer_resource_snapshot (
    client_pubkey_b64 TEXT PRIMARY KEY,
    cpu_percent REAL,
    ram_percent REAL,
    gpu_percent REAL,
    ram_used_bytes INTEGER,
    storage_used_bytes INTEGER,
    reported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE grpc_proxy_traffic_hourly (
    client_pubkey_b64 TEXT NOT NULL,
    hour_start TEXT NOT NULL,
    bytes_in INTEGER NOT NULL DEFAULT 0,
    bytes_out INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (client_pubkey_b64, hour_start)
);

CREATE INDEX idx_grpc_proxy_traffic_hour ON grpc_proxy_traffic_hourly (hour_start DESC);

CREATE TABLE server_resource_benchmark (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    run_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    cpu_score INTEGER NOT NULL,
    ram_score INTEGER NOT NULL,
    storage_score INTEGER NOT NULL,
    gpu_score INTEGER,
    raw_json TEXT NOT NULL DEFAULT ''
);
