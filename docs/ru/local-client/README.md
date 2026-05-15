# Local Client (RU)

Локальный клиентский контур отвечает за работу оператора на ПК: сборка артефакта, подключение к серверу по gRPC и управление деплоем.

## Модули

- [`local-stack/client`](../../../local-stack/client/README.md) - CLI для pair/status/upload/deploy.
- [`local-stack/desktop-client`](../../../local-stack/desktop-client/README.md) - Rust-логика desktop-команд для Tauri.
- [`local-stack/desktop-ui`](../../../local-stack/desktop-ui/README.md) - UI приложения `pirate-client`.
- [`local-stack/local-agent`](../../../local-stack/local-agent/README.md) - локальный агентный модуль (расширение сценариев).

## Основной поток

1. Собрать проект и артефакт на ПК.
2. Выполнить pair с сервером и проверить статус.
3. Загрузить артефакт и запустить deploy через gRPC/control API.

## Версия `pirate` в терминале

После обновления приложения `pirate --version` в shell может показывать старый `client=`, если в `PATH` осталась прежняя копия бинарника. Диагностика и решение: [client README (раздел Pirate CLI version and PATH)](../../../local-stack/client/README.md#pirate-cli-version-and-path).
