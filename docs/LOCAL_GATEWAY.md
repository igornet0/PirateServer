# Local PC gateway (design note)

This document describes how a **separate** “local PC ↔ remote server” connectivity layer fits next to the existing **deploy CLI** (`client deploy|status|rollback`), which speaks **gRPC** to `deploy-server` over IPv6.

## Why keep it separate

- **`client` (deploy CLI)** is an operator tool for artifact upload and release management. It should stay a thin gRPC client over `DeployService`.
- A **local gateway** (agent on a workstation, tunnel, or VPN) solves a different problem: reachability, NAT traversal, and trust boundaries between a user machine and a host running pirate stack. Mixing that into the deploy CLI blurs security and release workflows.

## Threat model (sketch)

| Concern | Mitigation direction |
|--------|----------------------|
| Confidentiality / integrity on the wire | TLS 1.3; for gRPC, **TLS + optional mTLS** (client certs issued by your CA). |
| Authenticating the server | Pin server cert or trust anchor; SNI + hostname checks. |
| Authenticating the client | mTLS, or application-level token after TLS (not a substitute for transport security on untrusted networks). |
| Blast radius | Do not expose raw `deploy-server` gRPC on the public Internet without TLS and auth; prefer VPN or SSH tunnel for admin access. |

## Implementation options (choose one per deployment)

1. **VPN (WireGuard / Tailscale / site-to-site)**  
   Run `client` and browsers against **private** addresses. No change to application protocol; enforce firewall so gRPC and `control-api` are not on `0.0.0.0`/`::` without intent.

2. **SSH port forwarding / reverse SSH**  
   `ssh -L` / `-R` to map local ports to `[::1]:50051` / `8080` on the server. Good for ad-hoc admin; not a full product feature but often enough.

3. **TLS-terminated reverse proxy**  
   nginx or Envoy in front of `deploy-server` with **gRPC over HTTP/2**, TLS certificates, optional client cert verification. `client` would use `https://host:443` with TLS config (future CLI flags).

4. **Dedicated small agent (future crate)**  
   A long-lived process on the PC that maintains an **outbound** connection to a broker or the server (useful behind strict egress). Protocol choice: **QUIC**, **WebSocket + protobuf**, or **gRPC** over pre-authenticated tunnel. This is a new binary; it does not replace `client`.

## Minimal MVP recommendation

- **Do not** implement a custom tunnel until VPN or SSH port-forward is insufficient.
- **Do** add **TLS for gRPC** (and optional mTLS) when exposing deploy endpoints beyond loopback; document certificate layout in the same style as `docs/PHASE6.md` for HTTP.

## References in this repo

- gRPC API: [`proto/deploy.proto`](../proto/deploy.proto) — `DeployService`.
- HTTP dashboard API: [`control-api`](../server-stack/control-api/src/main.rs) — `/api/v1/*`.
- IPv6 and ports: [`docs/PHASE6.md`](PHASE6.md).
