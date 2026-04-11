-- Allow postgresql rows in data_sources (saved connection; same payload as other SQL engines).

ALTER TABLE data_sources DROP CONSTRAINT IF EXISTS data_sources_kind_check;
ALTER TABLE data_sources DROP CONSTRAINT IF EXISTS data_sources_kind_payload;

ALTER TABLE data_sources ADD CONSTRAINT data_sources_kind_check CHECK (
  kind IN ('smb', 'clickhouse', 'oracle', 'mysql', 'mssql', 'mongodb', 'redis', 'postgresql')
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
    kind IN ('clickhouse', 'oracle', 'mysql', 'mssql', 'mongodb', 'redis', 'postgresql')
    AND config_json IS NOT NULL
    AND jsonb_typeof(config_json) = 'object'
    AND length(btrim(config_json->>'host')) > 0
  )
);
