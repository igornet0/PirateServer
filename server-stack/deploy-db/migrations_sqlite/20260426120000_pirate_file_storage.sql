-- Global file storage byte counter (Pirate file manager module, control-api).
CREATE TABLE IF NOT EXISTS pirate_file_storage_stats (
  id INTEGER NOT NULL CHECK (id = 1) PRIMARY KEY,
  used_bytes INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO pirate_file_storage_stats (id, used_bytes) VALUES (1, 0);
