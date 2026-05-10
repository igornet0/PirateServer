# Desktop UI: host database content authentication

## Behavior

- **Per `instance_id`** (host DB viewer): the Pirate desktop app stores **username**, **remember** flag, and **AES-256-GCM–encrypted password** in a local JSON file next to other Pirate desktop data:
  - Default directory: `{data_local_dir}/PirateClient/`
  - Files: `host_db_credentials.json` (payload), `host_db_credentials.key` (32-byte symmetric key), `host_db_credentials.lock` (cross-process lock).
  - Optional override for tests/tools: env `PIRATE_DESKTOP_DATA_DIR` (non-empty path replaces `PirateClient` root).
- **Per HTTP request** to `control-api` host-databases APIs, the Tauri side sends headers `x-pirate-db-user` and `x-pirate-db-password`. The **server does not persist** these; they are used only to build ephemeral DSN/connections for that request.
- **Viewing** schemas/tables/rows/Redis/Mongo/ClickHouse SQL in `DatabasesPanel` is **gated** until a non-empty **username** is set and a **password** is available (typed in the session, or decrypted from the local file when the user left the password field empty and a saved password exists).

## Direct DB profiles (separate feature)

Local “direct” connections in the DB explorer still use SQLite for profile metadata and the OS keychain for profile passwords (`PirateClient.db_direct_password`). That path is unrelated to host DB `instance_id` credentials above.

## Rollout (staged)

1. **Dev**: ship desktop build with the gate enabled (default). Verify login + browse + `Forget` clears stored credentials and blocks further browsing until new credentials.
2. **Internal**: same; collect feedback on file permissions and any OS security prompts.
3. **Optional fallback**: in `desktop-ui`, set `VITE_DESKTOP_DB_AUTH_REQUIRED=0` in the environment used to build the web bundle so the panel passes **no** per-request DB headers and the server uses its existing DSN env (legacy behavior). Use only for a controlled rollback, not for production long-term.
4. **Production**: keep auth required; document that support must not ask users to share DB passwords in chat; passwords stay in the local encrypted store + per-request wire only over HTTPS to `control-api`.

## Smoke (manual)

- Without username: content area shows the blocked message; list of instances still loads.
- After **Save** with **Remember** and a password: reload the panel; with username pre-filled and empty password, browsing works (decrypt from file).
- **Forget**: browsing blocked until re-entering credentials; instance entry is removed from JSON.

## Tests

- Rust: `pirate-desktop` — `credentials_crypto` roundtrip / wrong-key; `db_credentials` save/get/forget and remember edge cases.
- E2E: not automated here; use the smoke steps above in a Tauri dev build.
