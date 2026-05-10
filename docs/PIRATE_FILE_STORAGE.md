# Pirate file storage (server + desktop)

## Server (control-api)

- Set **`PIRATE_STORAGE_ROOT`** to an absolute path (e.g. `/var/lib/pirate/file-storage`). The directory should exist and be writable by the `control-api` process. If unset, storage API routes are disabled (503 / “not configured”).
- **`PIRATE_STORAGE_MAX_BYTES`**: total quota in bytes. `0` = unlimited. Enforcement uses a full filesystem walk under the root (metadata row `pirate_file_storage_stats` is reconciled after mutations and on `GET /api/v1/storage/usage` when a DB is configured).
- **`PIRATE_STORAGE_MAX_UPLOAD_BYTES`**: per-file cap. `0` = use **`DEPLOY_MAX_UPLOAD_BYTES`**.

**Internal:** staged uploads go under `<root>/.pirate-tmp/` (not listed in the UI).

**API** (all require the same `Authorization: Bearer` as the rest of control-api, JWT or static token):

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/v1/storage/tree?path=` | List one directory (relative path, POSIX) |
| GET | `/api/v1/storage/usage` | Used / max / percent |
| POST | `/api/v1/storage/folders` | JSON `{"path":"a/b"}` create folder |
| DELETE | `/api/v1/storage/folders?path=&recursive=` | Remove folder |
| PATCH | `/api/v1/storage/folders` | JSON `{"from","to"}` rename/move (same as files) |
| POST | `/api/v1/storage/files` | multipart: fields `path`, `file` |
| GET | `/api/v1/storage/files/download?path=` | Octet-stream |
| DELETE | `/api/v1/storage/files?path=` | Remove file |
| PATCH | `/api/v1/storage/files` | JSON `{"from","to"}` rename/move |

## Automatic setup (install / OTA)

- On the host, **`/usr/local/lib/pirate/pirate-ensure-file-storage.sh`** (root) creates `PIRATE_STORAGE_ROOT` (default `/var/lib/pirate/file-storage`), `.pirate-tmp`, appends `PIRATE_STORAGE_ROOT=` to `/etc/pirate-deploy.env` if missing, and deletes **temp files older than 1 day** under `.pirate-tmp/`.
- It runs after **fresh `install.sh`** and after each **`pirate update`** when `pirate-apply-stack-bundle.sh` syncs `lib/pirate/`.
- From the build machine, **`make server-stack-ota-full DEPLOY_URL=http://…:50051`** or **`scripts/pirate-ota-linux-full.sh`** run `make dist-linux` and `pirate update` with stdin from `/dev/null` so `pirate update` does not block on TTY prompts (set **`PIRATE_UPDATE_*`** if you move from no-UI to UI bundle; see `local-stack/client/src/stack_update_prompt.rs`).

## Desktop (PirateClient)

- Sidebar: **Storage** (Хранилище), or from the right context panel: **Open storage** (when connected to a server).
- Requires control-api **login** (JWT) in Connection settings, same as other host operations.

## Database

- Migration: `pirate_file_storage_stats` in PostgreSQL and SQLite (see `deploy-db/migrations*`).

## Tests

- `deploy-control` / `pirate_storage` unit test for `walk` skipping `.pirate-tmp`.
- `deploy-db` SQLite test for `pirate_file_storage_add_used_delta` (in `pirate_file_storage.rs`).
