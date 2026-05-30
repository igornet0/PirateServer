# Docker integration (two containers)

1. **`listener`** — `stack-tun-api` with a seeded RequestBus profile `e2e-bus-listen`.
2. **`runner`** — `pirate tunnel` (local then public modes), upstream `python -m http.server`, and curls against the REST API.

From the PirateServer repo root:

```bash
docker compose -f server-stack/stack-tun-api/docker/integration/docker-compose.yml up --build --abort-on-container-exit
```

`runner` exits **0** if both tunnels receive the HTTP task markers. Non‑zero exits fail the compose run (`--abort-on-container-exit`).

Runtime image installs **`libxcb1`** because the `pirate` binary links `xcap` (screen enumeration); without it `pirate` cannot start inside slim Debian.
