# PireteDocker / deploy workspace

Монорепозиторий: **серверный стек** (приём артефактов по gRPC, HTTP control plane, PostgreSQL, дашборд) и **локальные инструменты** (CLI `client`, заготовка `local-agent`).

## Быстрая навигация

| Что нужно | Куда смотреть |
|-----------|----------------|
| Роли каталогов (сервер / ПК / общее) | [`docs/ARCHITECTURE_SERVER_VS_LOCAL.md`](docs/ARCHITECTURE_SERVER_VS_LOCAL.md) |
| Локальный контур (проекты, UI на ПК, агент) | [`docs/LOCAL_STACK_DESIGN.md`](docs/LOCAL_STACK_DESIGN.md) |
| Локальный desktop UI (`pirate-client`, 127.0.0.1) | [`docs/DESKTOP_CLIENT.md`](docs/DESKTOP_CLIENT.md) |
| Сборка и цели Makefile | [`Makefile`](Makefile) |
| Стек nginx + PostgreSQL + UI | [`docs/PHASE6.md`](docs/PHASE6.md) |
| Дорожная карта и gRPC | [`ROADMAP.md`](ROADMAP.md) |

## Структура workspace

- **`proto/`** — общий gRPC-контракт (`deploy-proto`).
- **`server-stack/`** — `deploy-server`, `control-api`, `deploy-control`, `deploy-db`, `deploy-core`, серверные скрипты и конфиги в `server-stack/deploy/`, веб-дашборд в `server-stack/frontend/`.
- **`local-stack/`** — `deploy-client` (бинарь `client`), `local-agent` (заглушка), [`desktop-ui`](local-stack/desktop-ui/) (SPA) и [`desktop-client`](local-stack/desktop-client/) (бинарь `pirate-client`: локальный веб-UI на loopback).

Сборка: `make build` или `make build-local` (Rust + server dashboard). Локальный Pirate Client: `make pirate-desktop-all` (см. [`docs/DESKTOP_CLIENT.md`](docs/DESKTOP_CLIENT.md)).
