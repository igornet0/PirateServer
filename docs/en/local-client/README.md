# Local Client (EN)

The local client stack powers operator workflows on a workstation: artifact packaging, gRPC server connectivity, and deployment control.

## Modules

- [`local-stack/client`](../../../local-stack/client/README.md) - CLI for pair/status/upload/deploy.
- [`local-stack/desktop-client`](../../../local-stack/desktop-client/README.md) - Rust desktop command backend for Tauri.
- [`local-stack/desktop-ui`](../../../local-stack/desktop-ui/README.md) - `pirate-client` UI application.
- [`local-stack/local-agent`](../../../local-stack/local-agent/README.md) - local agent module for future automation flows.

## Primary Flow

1. Build project artifacts on the workstation.
2. Pair with the target server and validate status.
3. Upload artifact and trigger deploy through gRPC/control API.
