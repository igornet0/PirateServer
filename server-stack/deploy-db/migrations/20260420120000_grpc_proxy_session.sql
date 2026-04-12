-- Managed proxy sessions (limits, tokens, per-session traffic).

CREATE TABLE grpc_proxy_session (
    session_id TEXT PRIMARY KEY,
    client_pubkey_b64 TEXT NOT NULL,
    board_label TEXT NOT NULL DEFAULT '',
    token_sha256_hex TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    policy_json TEXT NOT NULL DEFAULT '{}',
    bytes_in BIGINT NOT NULL DEFAULT 0,
    bytes_out BIGINT NOT NULL DEFAULT 0,
    active_ms BIGINT NOT NULL DEFAULT 0,
    last_activity_at TIMESTAMPTZ,
    first_open_at TIMESTAMPTZ,
    revoked BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX idx_grpc_proxy_session_pubkey ON grpc_proxy_session (client_pubkey_b64);
CREATE INDEX idx_grpc_proxy_session_token ON grpc_proxy_session (token_sha256_hex);
