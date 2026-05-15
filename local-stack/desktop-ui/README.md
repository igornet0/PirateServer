# local-stack/desktop-ui

Desktop application UI (`pirate-desktop-ui`) built with React, Vite, and Tauri.

## Responsibilities

- Operator-facing UI for connection, deployment, and monitoring flows.
- Invokes Rust backend commands from `local-stack/desktop-client`.
- Displays server/project state and process/resource telemetry.

## Troubleshooting: `pirate --version` still shows an old `client=`

The terminal uses whatever `pirate` is first on `PATH`, not necessarily the copy inside the `.app`. See [client README: Pirate CLI version and PATH](../client/README.md#pirate-cli-version-and-path).

## Related docs

- RU: [`docs/ru/local-client/README.md`](../../docs/ru/local-client/README.md)
- EN: [`docs/en/local-client/README.md`](../../docs/en/local-client/README.md)
