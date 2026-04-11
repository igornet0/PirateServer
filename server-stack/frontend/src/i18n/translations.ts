export const TRANSLATIONS = {
  en: {
    "page.dashboardTitle": "PirateServer — Deploy dashboard",
    "page.loginTitle": "PirateServer — Sign in",
    "brand.logoAria": "PirateServer",

    "dash.title": "Deploy dashboard",
    "dash.tagline": "Control API: status, releases, and history",
    "btn.logout": "Sign out",

    "lang.label": "Language",
    "lang.en": "English",
    "lang.ru": "Русский",

    "apiToken.label": "API Bearer token (optional override)",
    "apiToken.placeholder":
      "Session JWT if signed in; paste static token to override",
    "activeProject.label": "Active project",
    "activeProject.placeholder": "default",

    "tabs.sectionsAria": "Dashboard sections",
    "tab.status": "Status",
    "tab.process": "Process",
    "tab.inventory": "Inventory",
    "tab.nginx": "Nginx",

    "status.heading": "Current status",
    "loading": "Loading…",

    "process.heading": "Process control",
    "process.hint":
      "Rollback, stop, or restart the managed app via control-api (gRPC to deploy-server). Same bearer token as above.",

    "rollback.label": "Rollback to version",
    "rollback.placeholder": "e.g. v1.0.0",
    "btn.rollback": "Rollback",
    "btn.restart": "Restart process",
    "btn.stop": "Stop process",

    "inv.projects": "Projects",
    "inv.releases": "Releases on disk",
    "inv.history": "Deploy history",
    "inv.table.id": "id",
    "inv.table.deployRoot": "deploy root",

    "nginx.heading": "Nginx configuration",
    "nginx.helpHtml":
      "Requires control-api with <code>NGINX_CONFIG_PATH</code>. Optional <code>NGINX_ADMIN_TOKEN</code> — paste below for Save (or use <code>X-Nginx-Admin-Token</code> when API token is set).",
    "nginx.adminToken.label": "Admin Bearer token (optional)",
    "nginx.adminToken.placeholder": "Bearer token if NGINX_ADMIN_TOKEN is set",
    "nginx.listen": "Listen",
    "nginx.serverName": "server_name",
    "nginx.staticRoot": "Static root",
    "nginx.apiUpstream": "API upstream",
    "nginx.tls": "TLS",
    "nginx.sslCert": "ssl_certificate",
    "nginx.sslKey": "ssl_certificate_key",
    "nginx.placeholderListen": "[::]:443",
    "nginx.placeholderServerName": "example.com",
    "nginx.placeholderCert": "/etc/ssl/certs/fullchain.pem",
    "nginx.placeholderKey": "/etc/ssl/private/privkey.pem",
    "nginx.appendSnippet": "Append snippet to editor",
    "nginx.editorPlaceholder": "Load to fetch current config…",
    "nginx.load": "Load",
    "nginx.save": "Save & reload nginx",

    "login.blurb":
      "Sign in with the admin account from the server (or skip for open API / static token).",
    "login.username": "Username",
    "login.password": "Password",
    "login.signIn": "Sign in",
    "login.skip": "Skip (open API / manual token)",
    "login.advancedToggle": "Advanced: static bearer token",
    "login.apiTokenLabel": "API Bearer token (<code>CONTROL_API_BEARER_TOKEN</code>)",
    "login.apiTokenPlaceholder": "Optional; copied to dashboard when you skip",

    "login.err.username": "Enter username.",
    "login.err.login503":
      "Login unavailable (set CONTROL_API_JWT_SECRET and DATABASE_URL on control-api).",
    "login.err.http": "HTTP {status}",
    "login.err.invalidResponse": "Invalid response from server.",
    "login.err.saveSession": "Could not save session.",
    "login.err.network": "Network error",

    "lifecycle.err.noVersion": "Enter a version to roll back to.",
    "lifecycle.rollingBack": "Rolling back…",
    "lifecycle.stopping": "Stopping…",
    "lifecycle.restarting": "Restarting…",

    "nginx.disabled.placeholder":
      "NGINX_CONFIG_PATH not set on control-api — nginx editor disabled.",
    "nginx.disabled.apiMsg":
      "API: nginx config editing disabled (set NGINX_CONFIG_PATH).",
    "nginx.loadedPattern": "Loaded: {path}",
    "nginx.saving": "Saving…",
    "nginx.snippetDone":
      "Snippet appended to editor. Review, then Save & reload nginx.",
  },
  ru: {
    "page.dashboardTitle": "PirateServer — Панель развёртывания",
    "page.loginTitle": "PirateServer — Вход",
    "brand.logoAria": "PirateServer",

    "dash.title": "Панель развёртывания",
    "dash.tagline": "Control API: статус, релизы и история",
    "btn.logout": "Выйти",

    "lang.label": "Язык",
    "lang.en": "English",
    "lang.ru": "Русский",

    "apiToken.label": "Bearer-токен API (необязательная подмена)",
    "apiToken.placeholder":
      "JWT сессии после входа; вставьте статический токен для подмены",
    "activeProject.label": "Активный проект",
    "activeProject.placeholder": "default",

    "tabs.sectionsAria": "Разделы панели",
    "tab.status": "Статус",
    "tab.process": "Процесс",
    "tab.inventory": "Инвентарь",
    "tab.nginx": "Nginx",

    "status.heading": "Текущий статус",
    "loading": "Загрузка…",

    "process.heading": "Управление процессом",
    "process.hint":
      "Откат, остановка или перезапуск приложения через control-api (gRPC к deploy-server). Тот же bearer-токен, что выше.",

    "rollback.label": "Откат на версию",
    "rollback.placeholder": "напр. v1.0.0",
    "btn.rollback": "Откатить",
    "btn.restart": "Перезапустить процесс",
    "btn.stop": "Остановить процесс",

    "inv.projects": "Проекты",
    "inv.releases": "Релизы на диске",
    "inv.history": "История развёртываний",
    "inv.table.id": "id",
    "inv.table.deployRoot": "корень deploy",

    "nginx.heading": "Конфигурация Nginx",
    "nginx.helpHtml":
      "Нужен control-api с <code>NGINX_CONFIG_PATH</code>. Необязательно <code>NGINX_ADMIN_TOKEN</code> — вставьте ниже для «Сохранить» (или используйте <code>X-Nginx-Admin-Token</code>, если задан токен API).",
    "nginx.adminToken.label": "Админский Bearer-токен (необязательно)",
    "nginx.adminToken.placeholder": "Bearer-токен, если задан NGINX_ADMIN_TOKEN",
    "nginx.listen": "Listen",
    "nginx.serverName": "server_name",
    "nginx.staticRoot": "Корень статики",
    "nginx.apiUpstream": "Upstream API",
    "nginx.tls": "TLS",
    "nginx.sslCert": "ssl_certificate",
    "nginx.sslKey": "ssl_certificate_key",
    "nginx.placeholderListen": "[::]:443",
    "nginx.placeholderServerName": "example.com",
    "nginx.placeholderCert": "/etc/ssl/certs/fullchain.pem",
    "nginx.placeholderKey": "/etc/ssl/private/privkey.pem",
    "nginx.appendSnippet": "Добавить фрагмент в редактор",
    "nginx.editorPlaceholder": "«Загрузить», чтобы получить текущий конфиг…",
    "nginx.load": "Загрузить",
    "nginx.save": "Сохранить и перезагрузить nginx",

    "login.blurb":
      "Войдите учётной записью администратора с сервера (или пропустите для открытого API / статического токена).",
    "login.username": "Имя пользователя",
    "login.password": "Пароль",
    "login.signIn": "Войти",
    "login.skip": "Пропустить (открытый API / токен вручную)",
    "login.advancedToggle": "Дополнительно: статический bearer-токен",
    "login.apiTokenLabel": "Bearer-токен API (<code>CONTROL_API_BEARER_TOKEN</code>)",
    "login.apiTokenPlaceholder": "Необязательно; при «Пропустить» копируется в панель",

    "login.err.username": "Введите имя пользователя.",
    "login.err.login503":
      "Вход недоступен (задайте CONTROL_API_JWT_SECRET и DATABASE_URL на control-api).",
    "login.err.http": "HTTP {status}",
    "login.err.invalidResponse": "Некорректный ответ сервера.",
    "login.err.saveSession": "Не удалось сохранить сессию.",
    "login.err.network": "Ошибка сети",

    "lifecycle.err.noVersion": "Укажите версию для отката.",
    "lifecycle.rollingBack": "Откат…",
    "lifecycle.stopping": "Остановка…",
    "lifecycle.restarting": "Перезапуск…",

    "nginx.disabled.placeholder":
      "NGINX_CONFIG_PATH не задан на control-api — редактор nginx отключён.",
    "nginx.disabled.apiMsg":
      "API: редактирование конфига nginx отключено (задайте NGINX_CONFIG_PATH).",
    "nginx.loadedPattern": "Загружено: {path}",
    "nginx.saving": "Сохранение…",
    "nginx.snippetDone":
      "Фрагмент добавлен в редактор. Проверьте, затем «Сохранить и перезагрузить nginx».",
  },
} as const;

export type Locale = keyof typeof TRANSLATIONS;
export type MessageKey = keyof (typeof TRANSLATIONS)["en"];
