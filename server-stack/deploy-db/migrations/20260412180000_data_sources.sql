-- External data sources (SMB mounts) for the dashboard "Databases" panel.

CREATE TABLE IF NOT EXISTS data_sources (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind TEXT NOT NULL CHECK (kind = 'smb'),
    label TEXT NOT NULL,
    smb_host TEXT NOT NULL,
    smb_share TEXT NOT NULL,
    smb_subpath TEXT NOT NULL DEFAULT '',
    mount_point TEXT NOT NULL UNIQUE,
    credentials_path TEXT NOT NULL,
    mount_state TEXT NOT NULL DEFAULT 'pending' CHECK (
        mount_state IN ('pending', 'mounted', 'error', 'unmounted')
    ),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_data_sources_kind ON data_sources (kind);
CREATE INDEX IF NOT EXISTS idx_data_sources_created ON data_sources (created_at DESC);
