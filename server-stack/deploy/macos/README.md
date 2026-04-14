# Развёртывание Pirate server-stack на macOS

Серверный бандл собирается на машине с macOS: `make dist-macos` из корня репозитория (см. корневой `Makefile`). Артефакт: `dist/pirate-macos-amd64-*.tar.gz` или `pirate-macos-arm64-*.tar.gz`.

## Содержимое

- Бинарники: `deploy-server`, `control-api`, `client`, `pirate` в `bin/`.
- Статика дашборда: `share/ui/dist` (если сборка без `UI_BUILD=0`).
- `launchd/`: plist для `com.pirate.deploy-server` и `com.pirate.control-api`.
- Скрипты: `install.sh`, `uninstall.sh`, `purge-pirate-data.sh`, `lib/pirate/*`.
- Общие с Ubuntu шаблоны nginx и `env.example` подтягиваются при сборке из [`../ubuntu`](../ubuntu) по [`build-config.json`](build-config.json).

## Требования

- macOS с архитектурой, совпадающей с бинарниками в архиве.
- Xcode Command Line Tools (`openssl`, `dscl`, `launchctl`).
- Для `--nginx`: [Homebrew](https://brew.sh); скрипт установит `nginx` и `openssl` через `brew install`.

## Установка

```bash
tar xzf pirate-macos-*-*.tar.gz
cd pirate-macos-*
sudo ./install.sh --nginx --ui
```

Минимум (без веб-интерфейса и без nginx): `sudo ./install.sh`.

## Отличия от Ubuntu

- Службы: **launchd** (`/Library/LaunchDaemons/`), не systemd.
- Пакеты: **Homebrew**, не apt; опциональные установщики СУБД из Ubuntu (`install-postgresql.sh` и т.д.) в macOS-бандл **не входят**.
- SMB: автоматический mount из `pirate-smb-mount.sh` на macOS в v1 **не реализован** (скрипт завершается с сообщением); размонтирование в `pirate-smb-umount.sh` работает при стандартном `umount`.
- OTA обновление стека: используется macOS-версия `pirate-apply-stack-bundle.sh` (launchctl, пути nginx Homebrew).

## Клиент в каталоге бандла

После установки бинарники в `/usr/local/bin`. В распакованном каталоге можно вызывать `make status` с `CLIENT=./bin/client` (вложенный `Makefile` копируется из `deploy/ubuntu` при сборке и совпадает по целям с Linux-бандлом).

## Удаление

```bash
sudo ./uninstall.sh
sudo ./purge-pirate-data.sh --remove-os-user
```

(Опции см. в скриптах.)
