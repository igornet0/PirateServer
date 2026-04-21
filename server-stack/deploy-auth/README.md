# server-stack/deploy-auth

Authentication and cryptographic primitives for deploy flows.

## Responsibilities

- Provides signing/verification helpers for pairing and transport security.
- Centralizes auth-related types used by client and server modules.
- Keeps security-sensitive primitives out of transport-specific crates.

## Related docs

- RU: [`docs/ru/server-client/README.md`](../../docs/ru/server-client/README.md)
- EN: [`docs/en/server-client/README.md`](../../docs/en/server-client/README.md)
