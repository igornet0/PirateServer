# stack-tun-api

HTTP control plane (**`:9380`**) plus gRPC data plane (**`:9381`**) coordinating:

- **TCP relay** (`TunnelStream`) — public TCP queued to a connector.
- **Synchronous request bus** (`RequestBusStream`) — policy + journaling.
- **Asynchronous HTTP task queue** (`TaskQueueStream`) — submits via REST or route rule `decision: "queue"`; workers claim envelopes and complete asynchronously.

Queues are **in-memory only**. Restart clears pending tasks.

## Task queue REST

Authenticated like other `/api/v1/*` routes when `STACK_TUN_REST_BEARER` is set.

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/tasks` | Enqueue `{ profileId, method, scheme, host, path, headers, bodyBase64, … }`. Returns **202** `{"requestId","phase":"queued"}`. Query `waitMs=` long-polls (max 120s) until terminal phase. |
| `GET` | `/api/v1/tasks?profileId=…` | Snapshot list (`phase`, `limit`). |
| `GET` | `/api/v1/tasks/:requestId?profileId=…` | Snapshot one task. |

## Routing: `queue` decision

Add a route rule with `"decision": "queue"` (camelCase JSON as stored in `/api/v1/routes`). Matched synchronous bus invokes enqueue instead of outbound HTTP.

## Worker CLI (`pirate tunnel`)

Runs on any host with outbound access to `:9381` (and `:9380` for setup).

Examples:

```bash
# Bearer token matches STACK_TUN_REST_BEARER on the stack-tun host.
pirate tunnel --url http://server:9380 --bearer "$STACK_TUN_REST_BEARER" \
  --listen-profile-id <listen-request-bus-uuid> --target 127.0.0.1:8080

# Restrict connector allow-list + authorized peer (recommended for WAN).
pirate tunnel --url https://example.com:9380 --mode public …
```

Daemon **TCP** connectors (`mode: tcpRelay`) still use the background `TunnelStream` loop; only `TcpRelay` profiles use `TunnelStream`. `TaskQueueStream` workers are external (CLI).

## Smoke (single host dev)

Requires a **Listen + RequestBus** profile id on stack-tun (no public TCP listen needed).

1. Submit: `curl -sS -H "Authorization: Bearer $STACK_TUN_REST_BEARER" -H "Content-Type: application/json" -d '{"profileId":"<id>","method":"GET","scheme":"http","host":"svc","path":"/ping","headers":{}}' http://127.0.0.1:9380/api/v1/tasks`
2. Run worker: `pirate tunnel --url http://127.0.0.1:9380 … --listen-profile-id <same-id> --target 127.0.0.1:<your-service>`
3. Poll: `curl -sS -H "Authorization: Bearer …" http://127.0.0.1:9380/api/v1/tasks/<requestId>?profileId=<id>`
