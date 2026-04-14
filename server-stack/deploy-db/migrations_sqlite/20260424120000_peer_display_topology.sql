CREATE TABLE IF NOT EXISTS peer_display_topology (
  client_pubkey_b64 TEXT PRIMARY KEY,
  updated_at_ms INTEGER NOT NULL,
  stream_capable INTEGER NOT NULL DEFAULT 0,
  json_displays TEXT NOT NULL DEFAULT '[]'
);
