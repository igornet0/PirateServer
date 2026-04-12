# PirateServer / deploy workspace

Монорепозиторий: **серверный стек** (приём артефактов по gRPC, HTTP control plane, метаданные в SQLite на «железе» или PostgreSQL в Docker, дашборд) и **локальные инструменты** (CLI `client`, заготовка `local-agent`).

## Быстрая навигация

| Что нужно | Куда смотреть |
|-----------|----------------|
| Роли каталогов (сервер / ПК / общее) | [`docs/ARCHITECTURE_SERVER_VS_LOCAL.md`](docs/ARCHITECTURE_SERVER_VS_LOCAL.md) |
| Локальный контур (проекты, UI на ПК, агент) | [`docs/LOCAL_STACK_DESIGN.md`](docs/LOCAL_STACK_DESIGN.md) |
| Локальный desktop UI (`pirate-client`, Tauri) | [`docs/DESKTOP_CLIENT.md`](docs/DESKTOP_CLIENT.md) |
| Сборка и цели Makefile | [`Makefile`](Makefile) |
| Стек nginx + метаданные + UI | [`docs/PHASE6.md`](docs/PHASE6.md) |
| Дорожная карта и gRPC | [`ROADMAP.md`](ROADMAP.md) |

## Структура workspace

- **`proto/`** — общий gRPC-контракт (`deploy-proto`).
- **`server-stack/`** — `deploy-server`, `control-api`, `deploy-control`, `deploy-db`, `deploy-core`, серверные скрипты и конфиги в `server-stack/deploy/`, веб-дашборд в `server-stack/frontend/`.
- **`local-stack/`** — `deploy-client` (бинарь `client`), `local-agent` (заглушка), [`desktop-ui`](local-stack/desktop-ui/) (Vite SPA + `src-tauri`) и [`desktop-client`](local-stack/desktop-client/) (crate `pirate-desktop`: логика для Tauri-команд).

Сборка: `make build` или `make build-local` (Rust + server dashboard). Локальный Pirate Client: `make pirate-desktop` или `make pirate-desktop-bundle` (см. [`docs/DESKTOP_CLIENT.md`](docs/DESKTOP_CLIENT.md)).

### Версия релиза

- Единый номер дистрибутива задаётся в файле **[`VERSION`](VERSION)** в корне (SemVer, одна строка).
- **`make dist`** — после `cargo build --release` и сборки дашборда пишет [`dist/release-manifest.json`](dist/release-manifest.json) (версии крейтов, npm, git, время сборки).
- **`make dist-linux`** / **`make dist-arm64-linux`** — кладут тот же номер в `server-stack-manifest.json` внутри архива (`release`, `target`, `git`, `built_at`, …); имя архива: `pirate-linux-{amd64|aarch64}-<VERSION>-<дата>.tar.gz`.
- При bump релиза обновите **`VERSION`** и при необходимости поля `version` в [`server-stack/frontend/package.json`](server-stack/frontend/package.json) и [`local-stack/desktop-ui/package.json`](local-stack/desktop-ui/package.json), чтобы они совпадали с политикой релиза.

## Bare-metal и gRPC

- Установка из `dist/pirate-linux-amd64-<версия>-<дата>.tar.gz` (или `pirate-linux-aarch64-…`): [`server-stack/deploy/ubuntu/install.sh`](server-stack/deploy/ubuntu/install.sh) создаёт пользователя ОС **`pirate`** (в т.ч. группа **`sudo`**), поднимает `deploy-server` и `control-api` под этим пользователем, выполняет `control-api bootstrap-grpc-key` и записывает `GRPC_SIGNING_KEY_PATH` в `/etc/pirate-deploy.env`. В архив входит каталог **`lib/pirate/`** (скрипты SMB и установки СУБД) — копируется в **`/usr/local/lib/pirate/`**; копии **`uninstall.sh`** / **`purge-pirate-data.sh`** — в **`/usr/local/share/pirate-uninstall/`**, путь к распакованному бандлу сохраняется в **`/var/lib/pirate/original-bundle-path`**. Полный веб-стек (nginx + статика дашборда): `sudo ./install.sh --nginx --ui` после распаковки.
- Снятие стека: после такой установки на хосте доступно **`sudo pirate uninstall stack`** (или **`sudo client uninstall stack`**) — вызывается тот же сценарий, что и **`sudo /usr/local/share/pirate-uninstall/uninstall.sh`**; на установках **до** появления этой копии по-прежнему **`sudo ./uninstall.sh`** из каталога распаковки. Локальная очистка pairing/настроек CLI на ПК: **`pirate uninstall client`** (удаляет каталог конфигурации `pirate-client`, без остановки процессов — остановите вручную долгоживущие команды вроде **`pirate board`**).
- Сборка архива **без** статики дашборда: `make dist-linux UI_BUILD=0` или `make dist-arm64-linux UI_BUILD=0` — в корне бандла появляется `.bundle-no-ui`, каталога `share/ui` нет; `install.sh --ui`, `pirate_UI=1` и цели `make install-ui` / `install-all` из распаковки **запрещены** (только backend и при необходимости `sudo ./install.sh --nginx`).
- Операторский CLI **`client`** на сервере после установки требует **`client pair`** с бандлом из `journalctl -u deploy-server` (строка `install bundle`), иначе gRPC вернёт `missing metadata: x-deploy-pubkey`.
- Порт **50051** — это gRPC (HTTP/2), не обычный HTTP; проверять через `client` / `grpcurl`, а не через `curl` к URL.
- Для локальной отладки на сервере можно выставить **`DEPLOY_GRPC_ALLOW_UNAUTHENTICATED=1`** для `deploy-server` (только dev, не для продакшена). См. [`server-stack/deploy/ubuntu/env.example`](server-stack/deploy/ubuntu/env.example).

### Веб-дашборд (логин)

- Установка **без** флага **`--ui`** не спрашивает домен и учётку дашборда и **не** записывает **`CONTROL_UI_ADMIN_*`** и **`CONTROL_API_JWT_SECRET`** в **`/etc/pirate-deploy.env`**; **`control-api`** слушает **`127.0.0.1:8080`** (переменная **`CONTROL_API_BIND`**) и не требует Bearer для **`/api/v1/*`**. Для клиента **`client pair`** по JSON этого достаточно.
- С **`sudo ./install.sh --ui`** (при необходимости вместе с **`--nginx`**) **`install.sh`** спрашивает имя и пароль первого пользователя (Enter — **`admin`**, пароль случайный), если не заданы **`pirate_NONINTERACTIVE=1`** или **`pirate_UI_ADMIN_USERNAME`** / **`pirate_UI_ADMIN_PASSWORD`**. В env попадают **`CONTROL_UI_ADMIN_USERNAME`**, **`CONTROL_UI_ADMIN_PASSWORD`**, **`CONTROL_API_JWT_SECRET`**; пароль выводится в конце установки. Повторный запуск с **`--ui`** добавляет или обновляет эти строки; pair по gRPC не зависит от JWT.
- Дополнительных пользователей дашборда: [`server-stack/deploy/ubuntu/Makefile`](server-stack/deploy/ubuntu/Makefile) — **`make dashboard-user-add DASH_USER=… DASH_PASS=…`** или **`make dashboard-user-add-interactive`** (имеет смысл после включения JWT, например **`sudo ./install.sh --ui`**).
- `deploy-server` при старте с **`DEPLOY_SQLITE_URL`** или **`DATABASE_URL`** применяет миграции и, если заданы **оба** **`CONTROL_UI_ADMIN_USERNAME`** и **`CONTROL_UI_ADMIN_PASSWORD`**, создаёт/обновляет пользователя дашборда (повторная запись пароля из env без **`CONTROL_UI_ADMIN_PASSWORD_RESET=1`** отключена).
- `control-api` с **`CONTROL_API_JWT_SECRET`** и метаданными включает **`POST /api/v1/auth/login`** и **`Authorization: Bearer`** для **`/api/v1/*`** (дополнительно **`CONTROL_API_BEARER_TOKEN`**). Встроенный explorer PostgreSQL в UI — при **`DATABASE_URL`** или **`POSTGRES_EXPLORER_URL`**. См. [`docs/PHASE6.md`](docs/PHASE6.md).
