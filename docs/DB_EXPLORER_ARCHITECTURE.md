# DB Explorer (desktop) — architecture

## Modes

| Mode | Data path | When to use |
|------|-----------|-------------|
| **Direct (Tauri / Rust)** | React → `invoke` → `pirate-desktop` (`db_direct`) → native drivers (PostgreSQL, …) | Default for any host you can reach from the machine (local, VPN, port-forward, SSH tunnel). No server-side proxy. |
| **B — control-api (host DB viewer)** | React → `control_api_host_db_*` (REST) → deploy-control on the Pirate host | Managed Pirate hosts: catalog/tree/grid/SQL can be served from the host agent path already implemented in `host_databases_*` / v2 APIs. |

The two modes can coexist: same UI can switch the **connection source** (saved profile = direct; “Pirate host instance” = mode B when JWT is available).

## Engine matrix and phases

| Engine | Direct (Rust) | Mode B (control-api) | Phase |
|--------|-----------------|------------------------|--------|
| PostgreSQL | `sqlx` + `PgPool` per session | Yes (existing) | **1 — MVP** |
| MySQL | `sqlx` mysql | Yes (existing) | 2+ |
| Redis | `redis` crate | Yes (existing) | 2+ |
| ClickHouse | HTTP (`reqwest`) or sqlx if enabled | Yes (server) | 3+ |
| MongoDB | Driver or shell-sidecar (TBD) | Yes (server) | 3+ |

## Secrets and profiles

- **Passwords:** OS keyring (`PirateClient.db_direct_password`, key = profile `id`).
- **Metadata:** `db_direct_profile` and `db_direct_query_history` in the same SQLite store as the rest of the desktop app (`pirate_desktop.db`).

## Tunnels

- **Plain TCP forward:** `127.0.0.1:local → host:port` (multi-instance slot map; stop per id).
- **SSH local forward:** optional sidecar `ssh -N -L local:remoteHost:remotePort user@sshHost` when OpenSSH is available; status surfaced in UI.

## Statistics (PG direct)

- Sources are **SQL-only** (`pg_stat_*`, `pg_database_size`, optional `pg_stat_statements` if extension exists). **No** host CPU/RAM without an agent; UI labels the data source.

## References

- Read-only SQL policy aligned with `deploy-control` `is_readonly_sql` (`db_host.rs`).
- Plan: internal product plan “Tauri Direct DB Explorer” (do not edit that file from automation).
