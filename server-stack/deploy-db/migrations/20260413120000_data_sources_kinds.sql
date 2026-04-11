-- Extend data_sources for non-SMB connection kinds (Redis, MySQL, etc.).

ALTER TABLE data_sources
  ADD COLUMN IF NOT EXISTS config_json JSONB;

ALTER TABLE data_sources DROP CONSTRAINT IF EXISTS data_sources_kind_check;
ALTER TABLE data_sources DROP CONSTRAINT IF EXISTS data_sources_mount_state_check;

ALTER TABLE data_sources
  ALTER COLUMN smb_host DROP NOT NULL,
  ALTER COLUMN smb_share DROP NOT NULL,
  ALTER COLUMN smb_subpath DROP NOT NULL,
  ALTER COLUMN mount_point DROP NOT NULL,
  ALTER COLUMN credentials_path DROP NOT NULL;

-- Existing SMB rows keep non-null smb_*; new non-SMB rows use NULL smb fields + config_json.

ALTER TABLE data_sources ADD CONSTRAINT data_sources_kind_check CHECK (
  kind IN ('smb', 'clickhouse', 'oracle', 'mysql', 'mssql', 'mongodb', 'redis')
);

ALTER TABLE data_sources ADD CONSTRAINT data_sources_mount_state_check CHECK (
  mount_state IN ('pending', 'mounted', 'error', 'unmounted', 'connected', 'n/a')
);

ALTER TABLE data_sources ADD CONSTRAINT data_sources_kind_payload CHECK (
  (
    kind = 'smb'
    AND smb_host IS NOT NULL
    AND smb_share IS NOT NULL
    AND smb_subpath IS NOT NULL
    AND mount_point IS NOT NULL
    AND credentials_path IS NOT NULL
    AND (config_json IS NULL OR jsonb_typeof(config_json) = 'object')
  )
  OR
  (
    kind IN ('clickhouse', 'oracle', 'mysql', 'mssql', 'mongodb', 'redis')
    AND config_json IS NOT NULL
    AND jsonb_typeof(config_json) = 'object'
    AND length(btrim(config_json->>'host')) > 0
  )
);
