# Pirate server-stack on Windows

Серверный бандл: `make dist-windows` из корня репозитория (на Windows с MSVC или кросс-сборка — см. ниже). Артефакт: `dist/pirate-windows-amd64-<VERSION>-<date>.zip` (и arm64 при необходимости).

## Содержимое

- `bin\deploy-server.exe`, `control-api.exe`, `client.exe`, `pirate.exe`
- `share\ui\dist` при сборке с `UI_BUILD=1`
- `lib\pirate\*.ps1` — обёртки и OTA-скрипт
- Шаблоны nginx и `env.example` подтягиваются из [`../ubuntu`](../ubuntu) через [`build-config.json`](build-config.json)

## Требования

- Windows 10 / Server 2016 или новее, **x64** или **ARM64** (соответствует архиву)
- [Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) для MSVC-сборок
- Установка: **PowerShell от имени администратора**

## Установка

Распакуйте zip и выполните:

```powershell
cd pirate-windows-amd64
Set-ExecutionPolicy -Scope Process Bypass
.\Install.ps1 -Ui
```

Полный UI + напоминание про nginx:

```powershell
.\Install.ps1 -Nginx -Ui
```

Флаг `-Nginx` не ставит nginx автоматически; шаблоны лежат в каталоге `nginx\`. Настройте [nginx для Windows](https://nginx.org/en/docs/windows.html) вручную.

## Службы

Используются **планировщик заданий** (SYSTEM, при старте системы): `PirateDeployServer`, `PirateControlApi` (задержка ~45 с).

## OTA

Переменная окружения **`PIRATE_APPLY_STACK_SCRIPT`** может переопределить путь к [`lib/pirate/pirate-apply-stack-bundle.ps1`](lib/pirate/pirate-apply-stack-bundle.ps1). По умолчанию deploy-server вызывает скрипт под `Program Files\Pirate\...` (см. код сервера).

## Сборка

### На Windows

Установите `rustup target add x86_64-pc-windows-msvc` (или `aarch64-pc-windows-msvc`), MSVC Build Tools, затем `make dist-windows`.

### Кросс-сборка с Linux/macOS

Используйте [cargo-xwin](https://github.com/rust-cross/cargo-xwin) или сборку в CI на `windows-latest`. Полная кросс-сборка `pc-windows-msvc` с хоста Unix может требовать дополнительных шагов (например `libsqlite3-sys` / линкер).

### MSI серверного бандла

Цель `make dist-windows-msi` для **серверного** стека пока не реализована (см. Makefile). Для **десктопного** клиента используйте `make dist-client-windows-msi`.

## SMB

Автоматический SMB-mount из дашборда в v1 не реализован (`pirate-smb-mount.ps1` завершается с сообщением).

## Удаление

```powershell
.\Uninstall.ps1
```

## Клиент (десктоп, Tauri)

Отдельно: `make dist-client-windows` / `dist-client-windows-msi` — см. корневой `Makefile`.
