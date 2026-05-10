# Desktop DB workspace v2 (DBeaver-like)

## control-api environment

| Variable | Default | Purpose |
|----------|---------|---------|
| `CONTROL_API_HOST_DATABASES` | on | Master switch for all host-database routes. |
| `CONTROL_API_HOST_DB_WORKSPACE_V2` | off | Enables `/api/v2/host-databases/*` (metadata tree, grid, SQL jobs). |
| `CONTROL_API_HOST_DB_WRITE` | off | Allows `POST .../row-mutate` (PostgreSQL/MySQL structured writes). |
| `CONTROL_API_HOST_DB_SQL_JOBS` | off | Async SQL job (`POST/GET/DELETE .../sql-jobs`). |
| `CONTROL_API_HOST_DB_MIGRATIONS` | off | Read-only detection of Alembic / Flyway / Prisma / Django metadata tables. |
| `CONTROL_API_HOST_DB_ADMIN_CREATE` | off | `POST .../admin/create-database` (**PostgreSQL:** `X-Pirate-Db-*`, superuser or `CREATEDB`; **MySQL:** host `PIRATE_MYSQL_ADMIN_URL`). `.../admin/create-table` (**PostgreSQL:** host `PIRATE_POSTGRES_ADMIN_URL`). `.../admin/create-user` (**PostgreSQL:** `X-Pirate-Db-*`, superuser or `CREATEROLE`). |
| `CONTROL_API_HOST_DB_MIGRATION_RUN` | off | `POST .../migration-run` (whitelisted CLIs, cwd allowlist on host). |

## Endpoints (v2)

- `GET /api/v2/host-databases/capabilities` — feature flags for the UI (`migration_status`, `admin_create_database`, `migration_run` included).
- `GET /api/v2/host-databases/:instance_id/object-tree` — schemas → tables (engine-specific).
- `POST /api/v2/host-databases/:instance_id/grid` — filtered/sorted browse (PostgreSQL/MySQL).
- `POST /api/v2/host-databases/:instance_id/row-mutate` — insert/update/delete when `CONTROL_API_HOST_DB_WRITE=1`.
- `POST /api/v2/host-databases/:instance_id/sql-jobs` — queue read-only query; `GET`/`DELETE` same path with `job_id`.
- `GET/POST /api/v2/host-databases/:instance_id/migration-status` — query `?database=` or JSON `{ "database", "tools?" }` when `CONTROL_API_HOST_DB_MIGRATIONS=1` (headers `X-Pirate-Db-*` required). Optional `tools` is a comma-separated filter (e.g. `alembic,prisma,flyway`). Response tools include `current_version` when detectable.
- `POST /api/v2/host-databases/:instance_id/admin/create-database` — JSON body `{ "database", "owner?", "encoding?", "if_not_exists?" }` when `CONTROL_API_HOST_DB_ADMIN_CREATE=1`. **PostgreSQL:** headers `X-Pirate-Db-*` (caller must be superuser or have `CREATEDB`). **MySQL:** `PIRATE_MYSQL_ADMIN_URL` (owner/encoding ignored).
- `POST /api/v2/host-databases/:instance_id/admin/create-table` — JSON `{ "database", "schema", "table", "columns": [...], "if_not_exists?" }` (allowlisted column `data_type` only). **PostgreSQL:** uses `schema` + `table`. **MySQL:** table is created in `database`; `schema` in JSON should match `database` (server ignores duplicate meaning).
- `POST /api/v2/host-databases/:instance_id/admin/create-user` — JSON `{ "database", "username", "password? | generate_password", "schema?", "privileges?", "allow_schema_ddl?" }` — response may include one-time `password` when `generate_password` is true (**PostgreSQL only**; **headers** `X-Pirate-Db-*` required; connecting user must be superuser or have `CREATEROLE`).
- `POST /api/v2/host-databases/:instance_id/admin/delete-user` — JSON `{ "username", "drop_owned_all_databases?" }` (default: `true`) — `DROP OWNED` in each non-template database then `DROP ROLE` (**PostgreSQL**; same `X-Pirate-Db-*` rules; cannot delete the same role you connect as).
- `POST /api/v2/host-databases/:instance_id/migration-run` — JSON `{ "tool", "workdir" }` when `CONTROL_API_HOST_DB_MIGRATION_RUN=1` (no per-request DB password).

Per-request DB credentials: headers `X-Pirate-Db-User` / `X-Pirate-Db-Password` (unchanged from v1). **`admin/create-database` (PostgreSQL)** and **`admin/create-user`** require `X-Pirate-Db-*`. **Exception — no DB password headers:** `admin/create-table` (PostgreSQL host `PIRATE_POSTGRES_ADMIN_URL`), **MySQL** `create-database`, and `migration-run` (JWT only).

## Desktop

Tauri commands: `control_api_host_db_v2_*` (see `local-stack/desktop-client` and `desktop-ui` `src-tauri`), including `control_api_host_db_v2_migration_status_get_json` (optional `tools` argument), `control_api_host_db_v2_admin_create_database_json` (optional `if_not_exists`), `control_api_host_db_v2_migration_run_json`.

UI: `local-stack/desktop-ui/src/databases/HostDatabaseServerToolbar.tsx` (admin + migration status/run) and `DatabasesWorkspaceV2.tsx` (object tree, grid, SQL jobs) when the server advertises `workspace_v2`.

## Security

- Read-only SQL policy is enforced in `deploy-control` `db_host::is_readonly_sql` (keyword allow/deny list; not a full SQL parser).
- Audit logs use SQL fingerprints only for query endpoints (`pirate.db.audit`).
- For migration status / admin / run, see `docs/HOST_DB_MIGRATIONS_AND_ADMIN.md` (matrix and threat model).
