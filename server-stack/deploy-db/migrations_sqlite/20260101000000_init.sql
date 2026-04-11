-- Metadata parity with PostgreSQL final schema (native install / SQLite file).

CREATE TABLE deploy_events (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN ('upload', 'rollback', 'stop', 'restart')),
    version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    state_snapshot TEXT,
    project_id TEXT NOT NULL DEFAULT 'default'
);

CREATE INDEX idx_deploy_events_created ON deploy_events (created_at DESC);
CREATE INDEX idx_deploy_events_project_created ON deploy_events (project_id, created_at DESC);

CREATE TABLE project_snapshots (
    project_id TEXT PRIMARY KEY,
    current_version TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'stopped',
    last_error TEXT,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE dashboard_users (
    id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_dashboard_users_username ON dashboard_users (username);

CREATE TABLE data_sources (
    id TEXT NOT NULL PRIMARY KEY,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    smb_host TEXT,
    smb_share TEXT,
    smb_subpath TEXT,
    mount_point TEXT,
    credentials_path TEXT,
    mount_state TEXT NOT NULL DEFAULT 'pending',
    last_error TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    config_json TEXT
);

CREATE INDEX idx_data_sources_kind ON data_sources (kind);
CREATE INDEX idx_data_sources_created ON data_sources (created_at DESC);
