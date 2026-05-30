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
  /** Значение по умолчанию (из env.example / install defaults) */
  defaultValue?: string;
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
        defaultValue: "sqlite:///var/lib/pirate/deploy/deploy.db",
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
      {
        key: "PIRATE_POSTGRES_ADMIN_URL",
        label: "PostgreSQL admin URL (v2 DDL / create-table)",
        hint: "Суперпользователь кластера; не путать с POSTGRES_EXPLORER_URL",
        type: "string",
      },
      {
        key: "PIRATE_MYSQL_ADMIN_URL",
        label: "MySQL admin URL (v2 create DB/table)",
        hint: "Учётная запись с CREATE и т.п.",
        type: "string",
      },
      {
        key: "PIRATE_MIGRATION_CWD_ALLOWLIST",
        label: "Allowlist каталогов для migration CLI",
        hint: "Через запятую; при CONTROL_API_HOST_DB_MIGRATION_RUN",
        type: "string",
      },
      { key: "DEPLOY_ROOT", label: "Корень деплоя", defaultValue: "/var/lib/pirate/deploy", type: "string" },
      {
        key: "GRPC_ENDPOINT",
        label: "GRPC_ENDPOINT (локальный ремот на хосте)",
        hint: "Обычно loopback для вызовов с сервера",
        defaultValue: "http://[::1]:50051",
        type: "string",
      },
      { key: "RUST_LOG", label: "RUST_LOG", hint: "Например info, debug", defaultValue: "info", type: "string" },
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
        defaultValue: "536870912",
        type: "string",
      },
      {
        key: "DEPLOY_MAX_UPLOAD_BYTES",
        label: "Макс. размер артефакта проекта (байты)",
        hint: "gRPC и HTTP (control-api: chunked session + legacy multipart); совпадает с --max-upload-bytes deploy-server. Пример: 268435456 = 256 MiB. Задайте одинаково для deploy-server и control-api, затем перезапустите оба сервиса.",
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
      {
        key: "DEPLOY_QUIC_DATAPLANE",
        label: "QUIC data-plane (UDP)",
        hint: "Вкл/выкл QUIC transport для proxy",
        defaultValue: "1",
        type: "boolean",
      },
    ],
  },
  {
    id: "control_api",
    title: "control-api (HTTP)",
    vars: [
      { key: "CONTROL_API_PORT", label: "Порт control-api", defaultValue: "8080", type: "string" },
      {
        key: "CONTROL_API_BIND",
        label: "BIND адрес",
        hint: "127.0.0.1 или 0.0.0.0 при отсутствии nginx",
        defaultValue: "127.0.0.1",
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
        defaultValue: "28800",
        type: "string",
      },
      {
        key: "CONTROL_API_DEPLOY_CHUNK_BYTES",
        label: "Размер HTTP deploy chunk (байты)",
        defaultValue: "262144",
        type: "string",
      },
      {
        key: "CONTROL_API_DEPLOY_SESSION_CHUNK_BYTES",
        label: "Размер chunk upload-session (байты)",
        defaultValue: "1048576",
        type: "string",
      },
      {
        key: "CONTROL_API_DEPLOY_SESSION_TTL_SECS",
        label: "TTL upload-session deploy (секунды)",
        defaultValue: "3600",
        type: "string",
      },
      {
        key: "CONTROL_API_STORAGE_SESSION_CHUNK_BYTES",
        label: "Chunk size: storage upload-session (байты)",
        hint: "Файловое хранилище, resumable upload",
        defaultValue: "1048576",
        type: "string",
      },
      {
        key: "CONTROL_API_STORAGE_SESSION_TTL_SECS",
        label: "TTL storage upload-session (секунды)",
        defaultValue: "3600",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DATABASES",
        label: "API «Базы на хосте» (/api/v1/host-databases)",
        type: "boolean",
      },
      {
        key: "CONTROL_API_HOST_DB_MAX_QUERY_ROWS",
        label: "Host DB: max строк SQL-ответа",
        defaultValue: "5000",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DB_MAX_PREVIEW_LIMIT",
        label: "Host DB: лимит превью таблицы",
        defaultValue: "500",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DB_MAX_OFFSET",
        label: "Host DB: max OFFSET",
        defaultValue: "10000000",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DB_MAX_SQL_BYTES",
        label: "Host DB: max размер тела SQL (байты)",
        defaultValue: "200000",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DB_MAX_REDIS_PATTERN_BYTES",
        label: "Host DB: max длина паттерна Redis",
        defaultValue: "512",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_DB_WORKSPACE_V2",
        label: "Host DB workspace v2 (/api/v2/host-databases)",
        type: "boolean",
      },
      { key: "CONTROL_API_HOST_DB_WRITE", label: "Host DB: запись (v2)", type: "boolean" },
      { key: "CONTROL_API_HOST_DB_SQL_JOBS", label: "Host DB: async SQL jobs", type: "boolean" },
      {
        key: "CONTROL_API_HOST_DB_MIGRATIONS",
        label: "Host DB: migration metadata / UI status",
        type: "boolean",
      },
      {
        key: "CONTROL_API_HOST_DB_ADMIN_CREATE",
        label: "Host DB admin: create DB/user/table (v2)",
        type: "boolean",
      },
      {
        key: "CONTROL_API_HOST_DB_MIGRATION_RUN",
        label: "Host DB: запуск whitelisted migration CLI",
        type: "boolean",
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
        defaultValue: "/var/lib/pirate/db-mounts",
        type: "string",
      },
      {
        key: "PIRATE_STORAGE_ROOT",
        label: "Корень файлового хранилища",
        defaultValue: "/var/lib/pirate/file-storage",
        type: "string",
      },
      {
        key: "PIRATE_STORAGE_MAX_BYTES",
        label: "Лимит хранилища (байты, 0 = без лимита)",
        defaultValue: "0",
        type: "string",
      },
      {
        key: "PIRATE_STORAGE_MAX_UPLOAD_BYTES",
        label: "Лимит одного файла (байты)",
        hint: "0 = взять DEPLOY_MAX_UPLOAD_BYTES",
        defaultValue: "0",
        type: "string",
      },
      {
        key: "PIRATE_STORAGE_BIND_SOURCE_PREFIXES",
        label: "Bind storage: префиксы путей-источников",
        hint: "Через «:» для pirate-storage-bind.sh; по умолчанию /mnt:/media:/srv",
        defaultValue: "/mnt:/media:/srv",
        type: "string",
      },
      {
        key: "PIRATE_STORAGE_BIND_SKIP_FAT_REMOUNT",
        label: "Bind storage: не делать vfat/exfat remount",
        hint: "1 — отключить remount (EIO на exfat после обновления)",
        type: "boolean",
      },
      {
        key: "PIRATE_STORAGE_BIND_FAT_REMOUNT_FMASK",
        label: "Bind storage: попробовать fmask/dmask после uid/gid",
        type: "boolean",
      },
      {
        key: "CONTROL_API_HOST_TERMINAL",
        label: "Host terminal через WebSocket",
        type: "boolean",
      },
      {
        key: "CONTROL_API_HOST_TERMINAL_SHELL",
        label: "Shell для host terminal",
        type: "string",
      },
      {
        key: "CONTROL_API_HOST_TERMINAL_SESSION_SECS",
        label: "TTL host terminal сессии (секунды)",
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
        defaultValue: "/etc/nginx/sites-available/pirate",
        type: "string",
      },
      {
        key: "CONTROL_API_NGINX_ENSURE_SCRIPT",
        label: "Скрипт ensure nginx (sudo)",
        defaultValue: "/usr/local/lib/pirate/pirate-ensure-nginx.sh",
        type: "string",
      },
      {
        key: "CONTROL_API_NGINX_APPLY_SITE_SCRIPT",
        label: "Скрипт apply site nginx (sudo)",
        defaultValue: "/usr/local/lib/pirate/pirate-nginx-apply-site.sh",
        type: "string",
      },
      {
        key: "CONTROL_API_NGINX_OPS_SCRIPT",
        label: "Скрипт ops nginx (test/reload)",
        defaultValue: "/usr/local/lib/pirate/pirate-nginx-ops.sh",
        type: "string",
      },
    ],
  },
  {
    id: "wan_security",
    title: "WAN security",
    vars: [
      {
        key: "PIRATE_WAN_DOMAIN",
        label: "WAN домен",
        type: "string",
      },
      {
        key: "PIRATE_WAN_ACME_EMAIL",
        label: "ACME email для WAN",
        type: "string",
      },
      {
        key: "PIRATE_WAN_FIREWALL_MANAGED",
        label: "Управление firewall для WAN",
        defaultValue: "1",
        type: "boolean",
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
  {
    id: "minio_meilisearch",
    title: "MinIO / Meilisearch (эндпоинты для приложений)",
    vars: [
      {
        key: "MINIO_HOST",
        label: "MinIO: хост API S3",
        hint: "Loopback по умолчанию; real credentials: /etc/pirate-minio.env",
        defaultValue: "127.0.0.1",
        type: "string",
      },
      {
        key: "MINIO_PORT",
        label: "MinIO: порт API (S3)",
        hint: "Совпадает с install-minio.sh (pirate-minio) — 9000",
        defaultValue: "9000",
        type: "string",
      },
      {
        key: "MINIO_CONSOLE_PORT",
        label: "MinIO: порт веб-консоли",
        defaultValue: "9001",
        type: "string",
      },
      {
        key: "MINIO_USE_SSL",
        label: "MinIO: HTTPS для S3 client",
        hint: "0 = http (loopback), 1 = https",
        defaultValue: "0",
        type: "string",
      },
      {
        key: "MINIO_ROOT_USER",
        label: "MinIO: root user (если копируете в env приложения)",
        type: "string",
      },
      {
        key: "MINIO_ROOT_PASSWORD",
        label: "MinIO: root password",
        type: "password",
      },
      {
        key: "MINIO_DATA_DIR",
        label: "MinIO: каталог данных (справка)",
        hint: "install-minio: /var/lib/pirate/minio",
        defaultValue: "/var/lib/pirate/minio",
        type: "string",
      },
      {
        key: "MEILI_HOST",
        label: "Meilisearch: HTTP host",
        defaultValue: "127.0.0.1",
        type: "string",
      },
      {
        key: "MEILI_PORT",
        label: "Meilisearch: HTTP порт",
        defaultValue: "7700",
        type: "string",
      },
      {
        key: "MEILI_MASTER_KEY",
        label: "Meilisearch: master key (если в env приложения)",
        hint: "Первичный серверный файл: /etc/pirate-meilisearch.env",
        type: "password",
      },
      {
        key: "MEILI_DB_PATH",
        label: "Meilisearch: путь к данным (справка)",
        hint: "install-meilisearch: /var/lib/pirate/meili/data",
        defaultValue: "/var/lib/pirate/meili/data",
        type: "string",
      },
    ],
  },
  {
    id: "stack_tun",
    title: "Stack tunnel API (stack-tun-api)",
    vars: [
      {
        key: "STACK_TUN_STATE_DIR",
        label: "Каталог состояния (очереди, журнал)",
        defaultValue: "/var/lib/pirate/stack-tun-api",
        type: "string",
      },
      {
        key: "STACK_TUN_HTTP_BIND",
        label: "HTTP control-plane (bind)",
        defaultValue: "127.0.0.1:9380",
        type: "string",
      },
      {
        key: "STACK_TUN_GRPC_BIND",
        label: "gRPC TunnelStream (bind)",
        defaultValue: "127.0.0.1:9381",
        type: "string",
      },
      {
        key: "STACK_TUN_AUTHORIZED_PEERS_PATH",
        label: "Файл authorized_peers.json",
        defaultValue: "/var/lib/pirate/stack-tun-api/authorized_peers.json",
        type: "string",
      },
      {
        key: "STACK_TUN_IDENTITY_PATH",
        label: "Файл identity.json",
        defaultValue: "/var/lib/pirate/stack-tun-api/identity.json",
        type: "string",
      },
      {
        key: "STACK_TUN_REST_BEARER",
        label: "Bearer для HTTP REST",
        type: "password",
      },
      {
        key: "STACK_TUN_ALLOW_UNAUTHENTICATED",
        label: "HTTP/gRPC без аутентификации",
        hint: "Только dev/test",
        type: "boolean",
      },
    ],
  },
  {
    id: "ssl",
    title: "SSL / Certbot",
    vars: [
      {
        key: "SSL_PROVIDER",
        label: "SSL provider",
        hint: "Обычно certbot",
        defaultValue: "certbot",
        type: "string",
      },
      {
        key: "SSL_EMAIL",
        label: "Email для Let's Encrypt",
        type: "string",
      },
      {
        key: "SSL_MODE",
        label: "Режим certbot",
        hint: "nginx | webroot | standalone | dns",
        type: "string",
      },
      {
        key: "SSL_WEBROOT",
        label: "Webroot путь",
        type: "string",
      },
      {
        key: "SSL_CERTBOT_DNS_PLUGIN",
        label: "DNS plugin certbot",
        type: "string",
      },
      {
        key: "SSL_CERTBOT_DNS_CREDENTIALS",
        label: "Путь к DNS credentials",
        type: "string",
      },
      {
        key: "SSL_CHECK_INTERVAL",
        label: "Интервал проверки SSL (секунды)",
        defaultValue: "86400",
        type: "string",
      },
      {
        key: "SSL_EXPIRY_THRESHOLD_DAYS",
        label: "Порог обновления до истечения (дни)",
        defaultValue: "7",
        type: "string",
      },
      {
        key: "SSL_ENABLE_SCHEDULER",
        label: "Автопланировщик renew",
        defaultValue: "1",
        type: "boolean",
      },
      {
        key: "SSL_CERTBOT_BIN",
        label: "Путь/имя certbot",
        defaultValue: "certbot",
        type: "string",
      },
      {
        key: "SSL_CERTBOT_EXTRA_ARGS",
        label: "Доп. аргументы certbot",
        type: "textarea",
      },
      {
        key: "SSL_USE_SUDO",
        label: "Запуск certbot через sudo",
        type: "boolean",
      },
      {
        key: "SSL_RELOAD_CMD",
        label: "Команда reload (без sudo)",
        type: "string",
      },
      {
        key: "PIRATE_NGINX_OPS_SCRIPT",
        label: "Скрипт nginx ops для SSL",
        type: "string",
      },
      {
        key: "SSL_ALERT_WEBHOOK_URL",
        label: "Webhook alert при ошибке renew",
        type: "string",
      },
      {
        key: "SSL_POST_CHECK_ENABLED",
        label: "Пост-проверка после issue/renew",
        defaultValue: "1",
        type: "boolean",
      },
      {
        key: "SSL_POST_CHECK_PATH",
        label: "Путь smoke-check",
        defaultValue: "/",
        type: "string",
      },
      {
        key: "SSL_POST_CHECK_PORT",
        label: "Порт smoke-check",
        defaultValue: "443",
        type: "string",
      },
      {
        key: "SSL_POST_CHECK_LOOPBACK",
        label: "Loopback для smoke-check",
        defaultValue: "127.0.0.1",
        type: "string",
      },
      {
        key: "SSL_POST_CHECK_HOST",
        label: "SNI host для smoke-check",
        type: "string",
      },
      {
        key: "SSL_STRICT_NGINX_RELOAD",
        label: "Strict nginx reload",
        type: "boolean",
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
