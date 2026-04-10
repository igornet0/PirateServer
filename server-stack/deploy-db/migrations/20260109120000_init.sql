-- Deploy audit + snapshot for control UI (source of truth remains FS + process).

CREATE TABLE IF NOT EXISTS deploy_events (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('upload', 'rollback')),
    version TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    state_snapshot TEXT
);

CREATE INDEX IF NOT EXISTS idx_deploy_events_created ON deploy_events (created_at DESC);

CREATE TABLE IF NOT EXISTS service_snapshot (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    current_version TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'stopped',
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO service_snapshot (id, current_version, state, last_error)
VALUES (1, '', 'stopped', NULL)
ON CONFLICT (id) DO NOTHING;
