/**
 * Описание переменных pirate-deploy.env (server-stack) для формы в desktop-ui.
 * Синхронизируйте с {@link ../../server-stack/deploy/ubuntu/env.example} при добавлении ключей.
 */

export type EnvFieldType = "string" | "password" | "boolean" | "textarea";

export type ServerEnvVarDef = {
  key: string;
  /** Короткий заголовок (RU) */
  label: string;
  /** Подсказка одной строкой */
  hint?: string;
  type: EnvFieldType;
};

export type ServerEnvCategory = {
  id: string;
  title: string;
  vars: ServerEnvVarDef[];
};

export const SERVER_DEPLOY_ENV_SCHEMA: ServerEnvCategory[] = [
  {
    id: "common",
    title: "Общие и метаданные",
    vars: [
      {
        key: "DEPLOY_SQLITE_URL",
        label: "SQLite (метаданные)",
        hint: "sqlite:///… для нативной установки",
        type: "string",
      },
      {
        key: "DATABASE_URL",
        label: "PostgreSQL (метаданные)",
        hint: "Вместо SQLite, если используете Postgres",
        type: "string",
      },
      {
        key: "POSTGRES_EXPLORER_URL",
        label: "PostgreSQL (explorer в UI)",
        hint: "Отдельная БД для встроенного explorer",
        type: "string",
      },
      { key: "DEPLOY_ROOT", label: "Корень деплоя", type: "string" },
      {
        key: "GRPC_ENDPOINT",
        label: "GRPC_ENDPOINT (локальный ремот на хосте)",
        hint: "Обычно loopback для вызовов с сервера",
        type: "string",
      },
      { key: "RUST_LOG", label: "RUST_LOG", hint: "Например info, debug", type: "string" },
    ],
  },
  {
    id: "deploy_server",
    title: "deploy-server (gRPC)",
    vars: [
      {
        key: "DEPLOY_GRPC_PUBLIC_URL",
        label: "Публичный gRPC URL",
        hint: "Как клиенты подключаются (LAN IP или https://…)",
        type: "string",
      },
      {
        key: "DEPLOY_CONTROL_API_PUBLIC_URL",
        label: "Публичный URL control-api (HTTP)",
        hint: "Без :8080 за nginx на :80/:443",
        type: "string",
      },
      {
        key: "DEPLOY_CONTROL_API_DIRECT_URL",
        label: "Прямой URL control-api",
        hint: "Часто http://127.0.0.1:8080",
        type: "string",
      },
      {
        key: "DEPLOY_ALLOW_SERVER_STACK_UPDATE",
        label: "OTA обновление server-stack",
        hint: "Разрешить загрузку бандла стека по gRPC",
        type: "boolean",
      },
      {
        key: "DEPLOY_MAX_SERVER_STACK_BYTES",
        label: "Макс. размер OTA tarball (байты)",
        type: "string",
      },
      {
        key: "DEPLOY_KEYS_DIR",
        label: "Каталог ключей",
        hint: "По умолчанию DEPLOY_ROOT/.keys",
        type: "string",
      },
      {
        key: "DEPLOY_GRPC_ALLOW_UNAUTHENTICATED",
        label: "gRPC без аутентификации",
        hint: "Только dev/test",
        type: "boolean",
      },
      {
        key: "DEPLOY_HOST_STATS_LOG_TAIL",
        label: "Лог-файл приложения (хвост в GetHostStats)",
        type: "string",
      },
      {
        key: "DEPLOY_PROXY_ALLOWLIST",
        label: "ProxyTunnel: allowlist хостов",
        hint: "Список через запятую или *",
        type: "textarea",
      },
    ],
  },
  {
    id: "control_api",
    title: "control-api (HTTP)",
    vars: [
      { key: "CONTROL_API_PORT", label: "Порт control-api", type: "string" },
      {
        key: "CONTROL_API_BIND",
        label: "BIND адрес",
        hint: "127.0.0.1 или 0.0.0.0 при отсутствии nginx",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DEPLOY_ENV_PATH",
        label: "Путь к файлу окружения на хосте",
        type: "string",
      },
      {
        key: "CONTROL_API_WRITE_DEPLOY_ENV_SCRIPT",
        label: "Скрипт записи env (sudo)",
        type: "string",
      },
      {
        key: "CONTROL_API_JWT_TTL_SECS",
        label: "TTL JWT (секунды)",
        type: "string",
      },
      {
        key: "GRPC_SIGNING_KEY_PATH",
        label: "Ключ подписи gRPC (control-api)",
        type: "string",
      },
      {
        key: "CONTROL_API_BEARER_TOKEN",
        label: "Статический Bearer",
        hint: "Автоматизация / машинные клиенты",
        type: "password",
      },
      {
        key: "CONTROL_API_CORS_ALLOW_ANY",
        label: "CORS: разрешить любые origin",
        type: "boolean",
      },
      {
        key: "CONTROL_API_CORS_ORIGINS",
        label: "CORS: список origin",
        hint: "Через запятую",
        type: "textarea",
      },
      {
        key: "CONTROL_API_HOST_STATS_SERIES",
        label: "История метрик хоста (графики)",
        type: "boolean",
      },
      {
        key: "CONTROL_API_HOST_STATS_STREAM",
        label: "Потоковая телеметрия хоста",
        type: "boolean",
      },
      {
        key: "CONTROL_API_LOG_TAIL_PATH",
        label: "Лог для host-stats (control-api)",
        type: "string",
      },
      {
        key: "PIRATE_DISPLAY_STREAM_CONSENT",
        label: "Согласие на display stream",
        type: "boolean",
      },
      {
        key: "PIRATE_DATA_MOUNTS_ROOT",
        label: "Корень для кредов БД / SMB",
        type: "string",
      },
    ],
  },
  {
    id: "subscriptions",
    title: "Подписки и ссылки",
    vars: [
      {
        key: "DEPLOY_SUBSCRIPTION_PUBLIC_HOST",
        label: "Публичный HTTPS для subscription / UI",
        type: "string",
      },
      {
        key: "DEPLOY_SUBSCRIPTION_TLS_SNI",
        label: "TLS SNI (ingress / Xray)",
        type: "string",
      },
      {
        key: "CONTROL_API_SUBSCRIPTION_PUBLIC_HOST",
        label: "Альтернатива DEPLOY_SUBSCRIPTION_PUBLIC_HOST",
        type: "string",
      },
      {
        key: "CONTROL_API_GRPC_PUBLIC_URL",
        label: "Дубль публичного gRPC для control-api",
        hint: "Иногда задают отдельно от deploy-server",
        type: "string",
      },
    ],
  },
  {
    id: "nginx",
    title: "Nginx (опционально)",
    vars: [
      { key: "NGINX_CONFIG_PATH", label: "Путь к nginx.conf", type: "string" },
      {
        key: "NGINX_TEST_FULL_CONFIG",
        label: "Полный тест конфига nginx",
        type: "boolean",
      },
      {
        key: "NGINX_ADMIN_TOKEN",
        label: "Токен admin API nginx",
        type: "password",
      },
      {
        key: "CONTROL_API_NGINX_SITE_PATH",
        label: "Путь к nginx site (pirate)",
        type: "string",
      },
      {
        key: "CONTROL_API_NGINX_ENSURE_SCRIPT",
        label: "Скрипт ensure nginx (sudo)",
        type: "string",
      },
      {
        key: "CONTROL_API_NGINX_APPLY_SITE_SCRIPT",
        label: "Скрипт apply site nginx (sudo)",
        type: "string",
      },
    ],
  },
  {
    id: "dashboard",
    title: "Веб-дашборд и учётные записи",
    vars: [
      {
        key: "CONTROL_UI_ADMIN_USERNAME",
        label: "Имя администратора UI",
        type: "string",
      },
      {
        key: "CONTROL_UI_ADMIN_PASSWORD",
        label: "Пароль администратора UI",
        type: "password",
      },
      {
        key: "CONTROL_API_JWT_SECRET",
        label: "Секрет JWT (HS256)",
        type: "password",
      },
      {
        key: "CONTROL_UI_ADMIN_PASSWORD_RESET",
        label: "Сбросить пароль сида при старте",
        type: "boolean",
      },
      {
        key: "DEPLOY_DASHBOARD_PASSWORD",
        label: "Пароль для dashboard-add-user (CLI)",
        type: "password",
      },
    ],
  },
];

const _keys = new Set<string>();
export const SERVER_DEPLOY_ENV_FLAT_KEYS: string[] = [];
for (const c of SERVER_DEPLOY_ENV_SCHEMA) {
  for (const v of c.vars) {
    _keys.add(v.key);
    SERVER_DEPLOY_ENV_FLAT_KEYS.push(v.key);
  }
}
export const SERVER_DEPLOY_ENV_KNOWN_KEYS: ReadonlySet<string> = _keys;
