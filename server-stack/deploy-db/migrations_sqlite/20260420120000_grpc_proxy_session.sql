-- Managed proxy sessions (limits, tokens, per-session traffic).

CREATE TABLE grpc_proxy_session (
    session_id TEXT PRIMARY KEY,
    client_pubkey_b64 TEXT NOT NULL,
    board_label TEXT NOT NULL DEFAULT '',
    token_sha256_hex TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    expires_at TEXT NOT NULL,
    policy_json TEXT NOT NULL DEFAULT '{}',
    bytes_in INTEGER NOT NULL DEFAULT 0,
    bytes_out INTEGER NOT NULL DEFAULT 0,
    active_ms INTEGER NOT NULL DEFAULT 0,
    last_activity_at TEXT,
    first_open_at TEXT,
    revoked INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_grpc_proxy_session_pubkey ON grpc_proxy_session (client_pubkey_b64);
CREATE INDEX idx_grpc_proxy_session_token ON grpc_proxy_session (token_sha256_hex);
