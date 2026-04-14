# Развёртывание Pirate server-stack на Linux (Ubuntu / Debian)

Исходники установки и unit-файлов лежат в этом каталоге; в **готовый архив** они попадают через сборку бандла из корня репозитория.

## Содержимое архива

После распаковки каталог `pirate-linux-amd64/` или `pirate-linux-aarch64/`:

| Путь | Назначение |
|------|------------|
| `bin/deploy-server`, `bin/control-api` | gRPC backend и HTTP API |
| `bin/client`, `bin/pirate` | один бинарник CLI (`pirate` — копия/симлинк на `client`) |
| `systemd/*.service` | unit-файлы для `deploy-server` и `control-api` |
| `nginx/*.conf`, `nginx/*.conf.in` | шаблоны nginx (сайт + API-only, с доменом и без) |
| `lib/pirate/*.sh` | OTA стека, SMB, опциональные установщики СУБД (`install-postgresql.sh` и т.д.) |
| `share/ui/dist/` | статика дашборда (если сборка не `UI_BUILD=0`) |
| `env.example` | комментированный шаблон переменных окружения |
| `install.sh`, `uninstall.sh`, `purge-pirate-data.sh` | установка и очистка |
| `Makefile` | цели `install-*`, `status`, `pair`, см. ниже |
| `server-stack-manifest.json` | манифест бандла для OTA |

## Требования на целевом хосте

- **ОС:** Debian/Ubuntu или совместимый дистрибутив с **systemd**.
- **Архитектура** должна совпадать с бинарниками в архиве (`x86_64` ↔ amd64-бандл, `aarch64` ↔ aarch64-бандл). Скрипт `install.sh` проверяет явное несоответствие ARM64 ↔ x86_64.
- **openssl** (для генерации секретов и паролей при `--ui`).
- Для полного веб-стека с **`--nginx`**: пакет **nginx** (скрипт установки поставит через `apt` при необходимости).
- Права **root** на время `install.sh` / `uninstall.sh`.

Опционально (через переменные окружения перед `install.sh`, см. `install.sh --help` / заголовок скрипта):

- `pirate_INSTALL_CIFS=1` — утилиты CIFS;
- `pirate_INSTALL_POSTGRESQL=1`, `pirate_INSTALL_MYSQL=1`, `pirate_INSTALL_REDIS=1`, и т.д. — вспомогательные установщики СУБД на хост.

## Установка

Распакуйте архив на сервере и перейдите в каталог бандла:

```bash
tar xzf pirate-linux-amd64-*.tar.gz
cd pirate-linux-amd64-*
```

Варианты:

| Сценарий | Команда |
|----------|---------|
| Только backend (SQLite, systemd, бинарники в `/usr/local/bin`) | `sudo ./install.sh` |
| + nginx (прокси `/api` на control-api) | `sudo ./install.sh --nginx` |
| + статика дашборда (без nginx веб с :80 не настраивается) | `sudo ./install.sh --ui` |
| Полный веб-стек | `sudo ./install.sh --nginx --ui` |

Полезные переменные (см. также комментарии в [`install.sh`](install.sh)):

- `pirate_NONINTERACTIVE=1` — без вопросов в TTY;
- `pirate_DOMAIN=deploy.example.com` или `--domain …` — домен для UI/nginx;
- `pirate_UI_ADMIN_USERNAME` / `pirate_UI_ADMIN_PASSWORD` — учётная запись дашборда при `--ui`;
- `pirate_DEPLOY_ALLOW_SERVER_STACK_UPDATE=0|1` — разрешение OTA обновления стека;
- `pirate_DISPLAY_STREAM_CONSENT=0|1` — политика трансляции экрана (после проверки GUI).

После установки:

- конфигурация: **`/etc/pirate-deploy.env`** (права обычно root + группа `pirate`, см. [`env.example`](env.example));
- данные: **`/var/lib/pirate/deploy`** и связанные каталоги;
- сервисы: `systemctl status deploy-server`, `systemctl status control-api`.

Шаблон всех переменных и пояснения — в [`env.example`](env.example). После смены сети или домена чаще всего нужно поправить **`DEPLOY_GRPC_PUBLIC_URL`** и при необходимости **`DEPLOY_SUBSCRIPTION_PUBLIC_HOST`**.

## Makefile в каталоге бандла

Удобные обёртки над `install.sh` и CLI:

```bash
sudo make install-all          # эквивалент install.sh --nginx --ui
sudo make install-clear
sudo make install-nginx
sudo make install-ui           # недоступно, если в архиве .bundle-no-ui
```

После установки (endpoint берётся из `/etc/pirate-deploy.env`):

```bash
sudo make status
sudo make print-bundle
sudo make pair BUNDLE=/path/to/bundle.json
```

До установки, из распакованного каталога:

```bash
make status CLIENT=./bin/client ENV_FILE=
```

## Удаление

```bash
sudo ./uninstall.sh
sudo ./purge-pirate-data.sh --remove-os-user   # при необходимости удалить данные и пользователя ОС
```

Опции см. в самих скриптах (`--help` / комментарии в начале файлов).
