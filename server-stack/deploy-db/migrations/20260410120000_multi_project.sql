-- Multi-project: per-project snapshots; deploy_events tagged by project.

ALTER TABLE deploy_events DROP CONSTRAINT IF EXISTS deploy_events_kind_check;

ALTER TABLE deploy_events
  ADD COLUMN IF NOT EXISTS project_id TEXT NOT NULL DEFAULT 'default';

ALTER TABLE deploy_events ADD CONSTRAINT deploy_events_kind_check
  CHECK (kind IN ('upload', 'rollback', 'stop', 'restart'));

CREATE INDEX IF NOT EXISTS idx_deploy_events_project_created
  ON deploy_events (project_id, created_at DESC);

CREATE TABLE IF NOT EXISTS project_snapshots (
    project_id TEXT PRIMARY KEY,
    current_version TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'stopped',
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO project_snapshots (project_id, current_version, state, last_error, updated_at)
SELECT 'default', current_version, state, last_error, updated_at
FROM service_snapshot WHERE id = 1
ON CONFLICT (project_id) DO UPDATE SET
  current_version = EXCLUDED.current_version,
  state = EXCLUDED.state,
  last_error = EXCLUDED.last_error,
  updated_at = EXCLUDED.updated_at;

INSERT INTO project_snapshots (project_id)
VALUES ('default')
ON CONFLICT (project_id) DO NOTHING;

DROP TABLE IF EXISTS service_snapshot;
