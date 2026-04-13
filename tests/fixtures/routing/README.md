# Фикстуры для e2e маршрутизации

- `rules-block.json` — `_block`: `test.block.local` → CONNECT **403**.
- `rules-pass.json` — `_pass`: `test.pass.local` → **direct** (без `ProxyTunnel`).
- `settings.json` — подключает оба файла; `boards.default.url` задаёт gRPC для сценариев с туннелем.
- `certs/` — самоподписанный сертификат для HTTPS upstream (`nginx`).
- `nginx.conf` — один `server` на 443 для имён из правил.

Запуск Docker e2e из корня репозитория:

```bash
./scripts/run-routing-e2e.sh
```

Требуется собранный образ `pirate-test-runtime` (как для `tests/docker/docker-compose.test.yml`).

Если после смены `tests/docker/docker-compose.routing-e2e.yml` всё ещё видите `authentication disabled; pairing unavailable`, пересоздайте контейнер `deploy-server` (в базовом `tests/docker/docker-compose.test.yml` включён анонимный gRPC; overlay отключает его):

`docker compose -f tests/docker/docker-compose.test.yml -f tests/docker/docker-compose.routing-e2e.yml down`

затем снова `./scripts/run-routing-e2e.sh`.
