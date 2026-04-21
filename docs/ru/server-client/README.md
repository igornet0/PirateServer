# Server Client (RU)

Серверный контур принимает деплой от локального клиента, сохраняет метаданные, выполняет выкладку и отдает управление через API/UI.

## Модули

- [`server-stack/server`](../../../server-stack/server/README.md) - основной gRPC-сервис deploy.
- [`server-stack/control-api`](../../../server-stack/control-api/README.md) - HTTP API для UI и desktop-клиента.
- [`server-stack/deploy-control`](../../../server-stack/deploy-control/README.md) - оркестрация deploy, файлов, nginx и БД.
- [`server-stack/deploy-db`](../../../server-stack/deploy-db/README.md) - слой БД и миграции.
- [`server-stack/frontend`](../../../server-stack/frontend/README.md) - веб-клиент панели управления.
- [`server-stack/deploy`](../../../server-stack/deploy/README.md) - скрипты установки и окружение bare-metal.

## Основной поток

1. Локальный клиент устанавливает pair и отправляет артефакт.
2. `server` и `deploy-control` валидируют и применяют релиз.
3. Состояние и метрики доступны через `control-api` и `frontend`.
