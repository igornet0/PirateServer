# Pirate Client — локальный desktop UI

Бинарь **`pirate-client`** (crate `pirate-desktop`) поднимает HTTP-сервер **только на `127.0.0.1`**, отдаёт статический SPA из [`local-stack/desktop-ui`](../local-stack/desktop-ui/) и при старте открывает браузер.

## Архитектура (текстовая схема)

```
┌─────────────────────────────────────────────────────────────┐
│  pirate-client (один процесс, Tokio + Axum)                  │
│  ┌──────────────┐   ┌─────────────────────────────────────┐ │
│  │ port::bind   │──►│ TcpListener 127.0.0.1:{90|9090|3k-9k} │ │
│  └──────────────┘   └─────────────────────────────────────┘ │
│  ┌──────────────┐   ┌─────────────────────────────────────┐ │
│  │ hosts::try_* │   │ optional append /etc/hosts (admin)  │ │
│  └──────────────┘   └─────────────────────────────────────┘ │
│  ┌──────────────┐   ┌─────────────────────────────────────┐ │
│  │ open::that   │──►│ system browser → preferred URL      │ │
│  └──────────────┘   └─────────────────────────────────────┘ │
│  ┌──────────────┐   ┌─────────────────────────────────────┐ │
│  │ Axum Router  │   │ GET /api/v1/status (JSON)            │ │
│  │              │   │ fallback: ServeDir → SPA index.html  │ │
│  └──────────────┘   └─────────────────────────────────────┘ │
│  ┌──────────────┐                                            │
│  │ tracing →    │  stdout + ~/…/PirateClient/logs/          │
│  └──────────────┘                                            │
└─────────────────────────────────────────────────────────────┘
         ▲
         │ resolve_ui_dir()
         │ 1) --ui-dir / PIRATE_DESKTOP_UI
         │ 2) <exe>/ui/index.html (portable layout)
         │ 3) debug: local-stack/desktop-ui/dist
```

## Сборка

```bash
make desktop-ui          # npm → local-stack/desktop-ui/dist
make pirate-desktop      # debug binary
# или одним шагом:
make pirate-desktop-all
```

Релиз: `make pirate-desktop-release` (после `desktop-ui`).

## Запуск

```bash
cargo run -p pirate-desktop --bin pirate-client
# без авто-браузера:
cargo run -p pirate-desktop --bin pirate-client -- --no-browser
```

Переменные:

| Переменная | Назначение |
|------------|------------|
| `PIRATE_DESKTOP_UI` | Каталог со статикой (`index.html` + `assets/`). |
| `PIRATE_SKIP_HOSTS=1` | Не трогать файл hosts. |
| `RUST_LOG` | Уровень логов (`info`, `debug`, …). |

## DNS / `pirate-client.internal`

- Целевая строка в hosts: `127.0.0.1 pirate-client.internal`.
- Запись добавляется **только если** процесс может писать в hosts (часто нужны права администратора/root).
- Если запись не создана: в UI и логах используется **`http://127.0.0.1:<port>`**; имя `pirate-client.internal` в URL не резолвится без ручной правки hosts.
- Порт **всегда** указывается в URL (кроме случая отдельного reverse-proxy на :80 — не входит в MVP).

## Порт

Порядок попыток привязки: **90 → 9090 → 3000…9999** на `127.0.0.1`.  
На Linux порты &lt;1024 обычно требуют capabilities/root — тогда 90 не поднимется и будет выбран следующий свободный.

## Поведение при сбоях

| Ситуация | Поведение |
|----------|-----------|
| Порт занят | Переход к следующему кандидату в списке. |
| Нет свободного порта в диапазоне | Ошибка, пауза 2 с, **авто-рестарт** внешнего цикла `main`. |
| Не удалось записать hosts | Предупреждение в лог; URL для браузера — `127.0.0.1`. |
| Браузер не открылся | `tracing::warn`, пользователь открывает URL из лога или UI вручную. |
| Ошибка сервера (кроме graceful shutdown) | Лог ошибки, пауза 2 с, рестарт. |
| Ctrl+C | Graceful shutdown `axum::serve`, процесс завершается без рестарта. |

## Упаковка (кроссплатформенно)

1. Собрать статику: `make desktop-ui`.
2. Собрать бинарь: `cargo build --release -p pirate-desktop --bin pirate-client`.
3. Упаковать в один каталог:

```
PirateClient/
  pirate-client.exe   # или pirate-client без расширения на Unix
  ui/                   # копия содержимого local-stack/desktop-ui/dist
```

Пользователь запускает `pirate-client`; разрешение UI берётся из `ui/` рядом с exe (см. `resolve_ui_dir` в `main.rs`).

Дополнительно:

- **Windows**: ярлык «Open Pirate Client», установщик Inno/NSIS/WiX — копирование `ui/` и бинаря.
- **macOS**: `.app` bundle: `Contents/MacOS/pirate-client`, `Contents/Resources/ui/`.
- **Linux**: tar.gz с `bin/pirate-client` и `share/pirate-client/ui/`, опционально `.desktop` файл с `Exec=`.

## Безопасность

- Слушаем только **loopback** (`127.0.0.1`), без `--host 0.0.0.0`.
- Внешняя сеть по умолчанию не задействована.

## Связанные документы

- [`LOCAL_STACK_DESIGN.md`](LOCAL_STACK_DESIGN.md) — контекст «локальный ПК» vs серверный стек.
