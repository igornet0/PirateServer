-- gRPC client session audit (pair, TCP lifecycle, optional RPC notes).

CREATE TABLE grpc_session_events (
    id BIGSERIAL PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    kind TEXT NOT NULL,
    client_pubkey_b64 TEXT,
    peer_ip TEXT NOT NULL DEFAULT '',
    grpc_method TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_grpc_session_events_created_id ON grpc_session_events (created_at DESC, id DESC);
CREATE INDEX idx_grpc_session_events_pubkey_id ON grpc_session_events (client_pubkey_b64, id DESC);
