-- Per enrolled client: last reported monitor list for display-stream UI.
CREATE TABLE IF NOT EXISTS peer_display_topology (
  client_pubkey_b64 TEXT PRIMARY KEY,
  updated_at_ms BIGINT NOT NULL,
  stream_capable BOOLEAN NOT NULL DEFAULT false,
  json_displays TEXT NOT NULL DEFAULT '[]'
);
