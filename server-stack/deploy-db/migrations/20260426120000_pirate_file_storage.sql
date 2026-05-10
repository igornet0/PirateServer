-- Global file storage byte counter (Pirate file manager module, control-api).
CREATE TABLE IF NOT EXISTS pirate_file_storage_stats (
  id INT PRIMARY KEY CHECK (id = 1),
  used_bytes BIGINT NOT NULL DEFAULT 0
);

INSERT INTO pirate_file_storage_stats (id, used_bytes) VALUES (1, 0) ON CONFLICT (id) DO NOTHING;
