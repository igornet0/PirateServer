# Host DB: migration status, admin create, migration run

## Capability matrix (what bypasses what)

| Path | Auth | DB credentials | Can run DDL | Can run shell / CLI | Notes |
|------|------|----------------|-------------|---------------------|--------|
| v1/v2 read-only query, grid, object-tree | JWT | `X-Pirate-Db-*` or explorer URL | No (`is_readonly_sql`) | No | See `deploy-control` `db_host`. |
| `POST .../migration-status` | JWT | `X-Pirate-Db-*` | No (SELECT metadata tables only) | No | `CONTROL_API_HOST_DB_MIGRATIONS=1`. |
| `POST .../admin/create-database` | JWT | `X-Pirate-Db-*` (PostgreSQL: superuser or `CREATEDB`) | Yes (`CREATE DATABASE`) | No | `CONTROL_API_HOST_DB_ADMIN_CREATE=1`, PostgreSQL; MySQL still uses host admin URL. |
| `POST .../admin/create-table` | JWT | None | Yes (`CREATE TABLE`, allowlisted types) | No | Same flag; identifiers validated. |
| `POST .../admin/create-user` | JWT | `X-Pirate-Db-*` (PostgreSQL; connecting user must be superuser or `CREATEROLE`) | Yes (`CREATE ROLE`…`GRANT`) | No | Same flag; does **not** use `PIRATE_POSTGRES_ADMIN_URL`; generated app password not logged in audit. |
| `POST .../admin/delete-user` | JWT | `X-Pirate-Db-*` (same; target role must differ from connecting user) | Yes (`DROP OWNED`…, `DROP ROLE`) | No | Same flag; optional `DROP OWNED` across all non-template DBs. |
| `POST .../migration-run` | JWT | None | No (app migration tools may run DDL *inside* the app DB) | Yes, **fixed** commands only | `CONTROL_API_HOST_DB_MIGRATION_RUN=1` + `PIRATE_MIGRATION_CWD_ALLOWLIST`. |

Pirate **metadata** migrations (`deploy-db`, deploy-server) are unrelated to application migration tools on the host.

## Threat model (short)

1. **Read-only migration status** — Risk is mostly information disclosure (schema tool in use, revision labels). Still require JWT and same DB creds as other host-db operations; audit logs record instance and database name, not passwords.
2. **Admin create database / create-user (PostgreSQL)** — High impact: JWT plus **`X-Pirate-Db-*`**: `CREATE DATABASE` runs as that user (must be superuser or have `CREATEDB`); `CREATE ROLE` requires superuser or `CREATEROLE`; neither reads `PIRATE_POSTGRES_ADMIN_URL`. **`create-table` (PostgreSQL)** still uses host `PIRATE_POSTGRES_ADMIN_URL` when set. `create-user` may return a generated app password **once** in the HTTP body (not written to `pirate.db.audit`). Store credentials in a password manager; restrict who receives JWT; keep host env `chmod 640`.
3. **Migration run** — Arbitrary code execution is avoided by **fixed** argv (`alembic upgrade head`, `npx prisma migrate deploy`, `flyway migrate`) and **directory allowlist** (`PIRATE_MIGRATION_CWD_ALLOWLIST`). Do not set allowlist to `/` or home directories. Prefer one project path per host.

## Pen-test checklist (internal)

- [ ] Cannot reach migration/admin/run endpoints when the corresponding `CONTROL_API_HOST_DB_*` flag is `0`.
- [ ] `migration-run` rejects `workdir` outside the canonicalized allowlist (including `..` attempts).
- [ ] `create-database` (PostgreSQL) fails closed without `X-Pirate-Db-*` or with a DB user that cannot create databases.
- [ ] `create-user` fails closed without `X-Pirate-Db-User` / `X-Pirate-Db-Password`, or with a DB user that cannot create roles.
- [ ] No password material in `pirate.db.audit` for these routes.

## Environment (host)

- `PIRATE_POSTGRES_ADMIN_URL` — e.g. `postgresql://postgres:SECRET@127.0.0.1:5432/postgres`
- `PIRATE_MIGRATION_CWD_ALLOWLIST` — comma-separated absolute paths, e.g. `/opt/app/my service`

## Environment (control-api)

- `CONTROL_API_HOST_DB_MIGRATIONS`
- `CONTROL_API_HOST_DB_ADMIN_CREATE`
- `CONTROL_API_HOST_DB_MIGRATION_RUN`

See `docs/DESKTOP_DB_WORKSPACE_V2.md` for endpoint list.
