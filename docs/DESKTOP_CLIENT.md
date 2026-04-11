# Pirate Client — локальный desktop UI (Tauri)

Приложение **`pirate-client`** — это **Tauri 2**: встроенный WebView, фронтенд из [`local-stack/desktop-ui`](../local-stack/desktop-ui/), общая Rust-логика в crate [`pirate-desktop`](../local-stack/desktop-client/) (команды `invoke`, без отдельного HTTP-сервера для UI).

## Архитектура

```
┌─────────────────────────────────────────────────────────────┐
│  pirate-client (Tauri)                                        │
│  ┌──────────────┐   ┌─────────────────────────────────────┐ │
│  │ WebView      │──►│ Vite-built SPA (dist / dev server)   │ │
│  └──────────────┘   └─────────────────────────────────────┘ │
│  ┌──────────────┐   ┌─────────────────────────────────────┐ │
│  │ invoke       │◄──│ get_status, connect_grpc_bundle, …       │
│  └──────────────┘   └─────────────────────────────────────┘ │
│  ┌──────────────┐                                            │
│  │ tracing →    │  stdout + ~/…/PirateClient/logs/          │
│  └──────────────┘                                            │
└─────────────────────────────────────────────────────────────┘
```

- **Нет** HTTP-сервера Axum на loopback для раздачи UI.
- **Нет** открытия системного браузера — окно приложения и есть UI.
- **Hosts:** только **чтение** (показ в `get_status`, есть ли строка `pirate-client.internal` в файле hosts); автозапись в hosts не выполняется.

## Сборка

```bash
make desktop-ui          # только Vite → local-stack/desktop-ui/dist
make pirate-desktop      # dist + cargo build -p pirate-client (debug)
make pirate-desktop-release
make pirate-desktop-all  # то же, что pirate-desktop
```

Полный **bundle** (`.app`, `.dmg`, `.msi`, … — по платформе):

```bash
make pirate-desktop-bundle
# или: cd local-stack/desktop-ui && npm install && npm run tauri:build
```

## Разработка

```bash
cd local-stack/desktop-ui
npm install
npm run tauri:dev
```

Откроется окно с dev-сервером Vite (`http://localhost:5174`); `invoke` ходит в тот же Rust-процесс.

## Версия релиза

- Репозиторий: файл [`VERSION`](../VERSION) в корне — единый SemVer для `make dist`, Linux-бандлов и поля `release` в `server-stack-manifest.json`.
- Vite подставляет `import.meta.env.VITE_APP_RELEASE` из `VERSION` (или переопределите `VITE_APP_RELEASE` при сборке).
- **Stack info** в UI: `bundleVersion` / `manifestJson` заполняются после установки бандла с манифестом или OTA; поле `deployServerBinaryVersion` всегда отражает версию бинаря `deploy-server` на хосте.

## Переменные окружения

| Переменная | Назначение |
|------------|------------|
| `RUST_LOG` | Уровень логов (`info`, `debug`, …). |
| `VITE_APP_RELEASE` | Опционально: переопределить строку релиза в UI (иначе читается из `VERSION`). |

## Команда `get_status`

Фронтенд вызывает `invoke('get_status')`. Ответ — JSON с полями вроде `hostname`, `hosts_entry_ok`, `shell` (`"tauri"`).

## Подключение к `deploy-server` (gRPC)

В UI есть кнопка **Connect…**: открывается форма, куда можно вставить **тот же блок**, что печатает [`scripts/print-docker-connection.sh`](../scripts/print-docker-connection.sh) после `docker compose up` (строка вида `export GRPC_ENDPOINT=http://127.0.0.1:50051`), или **одну строку** с URL `http://…` / `https://…`.

Rust:

- `parse_grpc_bundle` — извлекает URL из JSON-бандла (`token`, `url`, `pairing`) или legacy `export GRPC_ENDPOINT=…` / одна строка URL;
- `connect_grpc_bundle` — для JSON вызывает `Pair`, проверяет подпись сервера, сохраняет подключение в SQLite (`pirate_desktop.db`) и при необходимости создаёт `identity.json`; для legacy — только `GetStatus`;
- `refresh_grpc_status` / `clear_grpc_connection` — обновить статус по сохранённому endpoint или забыть его.

Аутентификация на gRPC в протоколе не задаётся — защищайте сеть или добавьте mTLS (см. [`GRPC_AUTH_FUTURE.md`](GRPC_AUTH_FUTURE.md)).

## OTA обновления server-stack (хост)

В дашборде есть блок **Server stack update**: выбор готового архива `pirate-linux-amd64*.tar.gz` или распакованной папки (как после [`scripts/build-linux-bundle.sh`](../scripts/build-linux-bundle.sh)), метка версии и загрузка по gRPC (`UploadServerStack`). Прогресс бара отражает **реальную** долю отправленных байт.

На стороне **deploy-server** нужно:

- `DEPLOY_ALLOW_SERVER_STACK_UPDATE=1` (или флаг `--allow-server-stack-update`);
- установка с [`server-stack/deploy/ubuntu/install.sh`](../server-stack/deploy/ubuntu/install.sh), чтобы существовали `/usr/local/lib/pirate/pirate-apply-stack-bundle.sh` и правило `sudoers` для пользователя `pirate`;
- при необходимости увеличить `DEPLOY_MAX_SERVER_STACK_BYTES` для больших бандлов с UI.

После применения `deploy-server` и `control-api` перезапускаются с задержкой; кратковременный обрыв gRPC ожидаем — обновите статус подключения через несколько секунд.

## Упаковка

Артефакты Tauri по умолчанию в `local-stack/desktop-ui/src-tauri/target/...` (member workspace) или в корневом `target/` при единой сборке — см. вывод `tauri build` / `make pirate-desktop-bundle`.

## Безопасность

- UI не открыт в сети; нет отдельного loopback HTTP API для статуса.
- После явного **Connect** приложение инициирует **исходящее** gRPC-подключение к указанному endpoint (тот же `DeployService`, что и клиент `client` на сервере).

## Связанные документы

- [`LOCAL_STACK_DESIGN.md`](LOCAL_STACK_DESIGN.md) — контекст «локальный ПК» vs серверный стек.
