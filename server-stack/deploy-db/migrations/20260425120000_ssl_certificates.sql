-- ACME / Certbot metadata (paths only; no private keys).

CREATE TABLE IF NOT EXISTS ssl_certificates (
    primary_domain TEXT NOT NULL PRIMARY KEY,
    cert_name TEXT NOT NULL,
    domains_json TEXT NOT NULL,
    live_path TEXT NOT NULL,
    expiry_utc_ms BIGINT NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'error' CHECK (status IN ('valid', 'expiring_soon', 'expired', 'error')),
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ssl_certificates_status ON ssl_certificates (status);
CREATE INDEX IF NOT EXISTS idx_ssl_certificates_expiry ON ssl_certificates (expiry_utc_ms);

CREATE TABLE IF NOT EXISTS ssl_renewal_events (
    id BIGSERIAL PRIMARY KEY,
    primary_domain TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK (event_kind IN ('check', 'renew', 'error', 'webhook', 'info')),
    message TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ssl_renewal_events_domain ON ssl_renewal_events (primary_domain, created_at DESC);
