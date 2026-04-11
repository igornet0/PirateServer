# PirateServer / deploy workspace

Монорепозиторий: **серверный стек** (приём артефактов по gRPC, HTTP control plane, PostgreSQL, дашборд) и **локальные инструменты** (CLI `client`, заготовка `local-agent`).

## Быстрая навигация

| Что нужно | Куда смотреть |
|-----------|----------------|
| Роли каталогов (сервер / ПК / общее) | [`docs/ARCHITECTURE_SERVER_VS_LOCAL.md`](docs/ARCHITECTURE_SERVER_VS_LOCAL.md) |
| Локальный контур (проекты, UI на ПК, агент) | [`docs/LOCAL_STACK_DESIGN.md`](docs/LOCAL_STACK_DESIGN.md) |
| Локальный desktop UI (`pirate-client`, Tauri) | [`docs/DESKTOP_CLIENT.md`](docs/DESKTOP_CLIENT.md) |
| Сборка и цели Makefile | [`Makefile`](Makefile) |
| Стек nginx + PostgreSQL + UI | [`docs/PHASE6.md`](docs/PHASE6.md) |
| Дорожная карта и gRPC | [`ROADMAP.md`](ROADMAP.md) |

## Структура workspace

- **`proto/`** — общий gRPC-контракт (`deploy-proto`).
- **`server-stack/`** — `deploy-server`, `control-api`, `deploy-control`, `deploy-db`, `deploy-core`, серверные скрипты и конфиги в `server-stack/deploy/`, веб-дашборд в `server-stack/frontend/`.
- **`local-stack/`** — `deploy-client` (бинарь `client`), `local-agent` (заглушка), [`desktop-ui`](local-stack/desktop-ui/) (Vite SPA + `src-tauri`) и [`desktop-client`](local-stack/desktop-client/) (crate `pirate-desktop`: логика для Tauri-команд).

Сборка: `make build` или `make build-local` (Rust + server dashboard). Локальный Pirate Client: `make pirate-desktop` или `make pirate-desktop-bundle` (см. [`docs/DESKTOP_CLIENT.md`](docs/DESKTOP_CLIENT.md)).

## Bare-metal и gRPC

- Установка из `dist/pirate-linux-amd64-*.tar.gz`: [`server-stack/deploy/ubuntu/install.sh`](server-stack/deploy/ubuntu/install.sh) поднимает `deploy-server`, создаёт ключ для `control-api` (`control-api bootstrap-grpc-key`) и записывает `GRPC_SIGNING_KEY_PATH` в `/etc/pirate-deploy.env`.
- Операторский CLI **`client`** на сервере после установки требует **`client pair`** с бандлом из `journalctl -u deploy-server` (строка `install bundle`), иначе gRPC вернёт `missing metadata: x-deploy-pubkey`.
- Порт **50051** — это gRPC (HTTP/2), не обычный HTTP; проверять через `client` / `grpcurl`, а не через `curl` к URL.
- Для локальной отладки на сервере можно выставить **`DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1`** для `deploy-server` (только dev, не для продакшена). См. [`server-stack/deploy/ubuntu/env.example`](server-stack/deploy/ubuntu/env.example).

### Веб-дашборд (логин)

- При установке [`install.sh`](server-stack/deploy/ubuntu/install.sh) интерактивно спрашивают имя и пароль первого пользователя (Enter — имя **`admin`**, пароль случайный); при **`pirate_NONINTERACTIVE=1`** или заданных **`pirate_UI_ADMIN_USERNAME`** / **`pirate_UI_ADMIN_PASSWORD`** используются значения по умолчанию или из env. В `/etc/pirate-deploy.env` пишутся **`CONTROL_UI_ADMIN_USERNAME`**, **`CONTROL_UI_ADMIN_PASSWORD`**, **`CONTROL_API_JWT_SECRET`**; пароли дашборда и БД выводятся в конце установки.
- Дополнительных пользователей дашборда: [`server-stack/deploy/ubuntu/Makefile`](server-stack/deploy/ubuntu/Makefile) — **`make dashboard-user-add DASH_USER=… DASH_PASS=…`** или **`make dashboard-user-add-interactive`** (вызывает **`deploy-server dashboard-add-user`**).
- `deploy-server` при старте с `DATABASE_URL` применяет миграции и создаёт/обновляет пользователя дашборда из **`CONTROL_UI_ADMIN_USERNAME`** / **`CONTROL_UI_ADMIN_PASSWORD`** (повторная запись пароля из env без **`CONTROL_UI_ADMIN_PASSWORD_RESET=1`** отключена).
- `control-api` с **`DATABASE_URL`** и **`CONTROL_API_JWT_SECRET`** включает **`POST /api/v1/auth/login`** и принимает заголовок `Authorization: Bearer` с JWT для `/api/v1/*` (дополнительно можно задать **`CONTROL_API_BEARER_TOKEN`** для машинного доступа). Без JWT-секрета и без static bearer API остаётся открытым, как раньше.
