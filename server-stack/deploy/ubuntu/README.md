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
| `lib/pirate/*.sh`, `lib/pirate/99-pirate-smb.sudoers.fragment` | OTA стека, SMB, установщики (`install-*.sh`) и **скрипты удаления (`remove-*.sh`)** для диспетчера `pirate-host-service.sh`, WAN helpers (`pirate-ensure-https.sh`, …); фрагмент — NOPASSWD как при `install.sh` |
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

Для WAN-режима используйте helper-скрипты:

```bash
sudo /usr/local/lib/pirate/pirate-ensure-https.sh example.com ops@example.com
sudo /usr/local/lib/pirate/pirate-firewall-apply.sh wan 50051
```

После установки:

- конфигурация: **`/etc/pirate-deploy.env`** (права обычно root + группа `pirate`, см. [`env.example`](env.example));
- данные: **`/var/lib/pirate/deploy`** и связанные каталоги;
- сервисы: `systemctl status deploy-server`, `systemctl status control-api`.

Шаблон всех переменных и пояснения — в [`env.example`](env.example). После смены сети или домена чаще всего нужно поправить **`DEPLOY_GRPC_PUBLIC_URL`** и при необходимости **`DEPLOY_SUBSCRIPTION_PUBLIC_HOST`**.

### Первое OTA после смены формата архива / рассинхрона распаковки

Команда **`pirate update`** / `UploadServerStack` распаковывает `.tar.gz` **внутри процесса `deploy-server`**. Логика распаковки и поиска корня бандла (`pirate-linux-aarch64/` и т.д.) живёт в **самом бинарнике** `deploy-server`. Если удалённый сервер ещё на старой сборке, OTA может снова падать с ошибкой вида «expected bundle with bin/deploy-server…», не исправляя себя по воздуху.

**Один раз** (bootstrap без цикла OTA):

1. На машине с доступом к хосту распакуйте тот же архив, которым пользуетесь для OTA: `tar xzf pirate-linux-aarch64-*.tar.gz`.
2. Скопируйте из `pirate-linux-aarch64/bin/` файлы **`deploy-server`** и при необходимости **`control-api`** в каталоги из unit-файлов systemd (см. `ExecStart` в [`deploy-server.service`](deploy-server.service) и [`control-api.service`](control-api.service)), обычно что-то вроде `/usr/local/bin/` или домашний каталог пользователя `pirate`.
3. `sudo systemctl restart deploy-server` (и `control-api`, если обновляли).
4. Убедитесь, что в **`/etc/pirate-deploy.env`** для OTA задано **`DEPLOY_ALLOW_SERVER_STACK_UPDATE=1`** (или эквивалент при установке).
5. Повторите `pirate update` с клиента.

Дальнейшие обновления стека через gRPC должны выполняться уже новым `deploy-server`, без ручного копирования бинарников на каждый релиз.

Проверить архив локально перед загрузкой: из корня репозитория [`scripts/verify-server-stack-bundle-tar.sh`](../../../scripts/verify-server-stack-bundle-tar.sh) — распаковывает во временный каталог и проверяет наличие обоих бинарников в ожидаемом корне.

**Два канала OTA:** обычное обновление — gRPC `UploadServerStack` к `deploy-server`. Если `deploy-server` или `control-api` не отвечают, используйте отдельный сервис **`pirate-host-agent`** (HTTP, токен в `/etc/pirate-host-agent.env`) и тот же `.tar.gz`; подробнее — [`docs/HOST_AGENT.md`](../../../docs/HOST_AGENT.md).

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

## Опциональные пакеты через десктоп (вкладка «Сервисы»)

После установки или OTA стека на хосте в `/usr/local/lib/pirate/` лежит **`pirate-host-service.sh`** — whitelist-диспетчер (`install` / `remove` для фиксированных id: `node`, `redis`, `postgresql`, …). В `install.sh` пользователю `pirate` выдаётся `NOPASSWD` только на этот скрипт (в дополнение к SMB/nginx/env). Если в ответе API или в логах видно `sudo: a password is required`, обновите хост (повторный `./install.sh` из актуального бандла или правка `/etc/sudoers.d/…`) и проверьте от имени `pirate`: `sudo -n /usr/local/lib/pirate/pirate-host-service.sh install node`.

**Preflight OTA:** перед `pirate update` убедитесь, что в архиве есть фрагмент sudoers (иначе OTA снова сузит NOPASSWD):

```bash
tar tzf pirate-linux-aarch64-no-ui-*.tar.gz | grep lib/pirate/99-pirate-smb.sudoers.fragment
```

Или: `./scripts/verify-server-stack-bundle-tar.sh dist/pirate-linux-*-no-ui-*.tar.gz`.

**Проверка на хосте после OTA:** `grep host-service /etc/sudoers.d/99-pirate-smb` и `sudo -u pirate sudo -n /usr/local/lib/pirate/pirate-host-service.sh --help` (без запроса пароля).

**Важно:** удаление СУБД через API вызывает сценарии `remove-*.sh` (`apt-get remove --purge`). Это может **безвозвратно удалить данные** на хосте. Используйте на тестовых машинах или после резервного копирования.
