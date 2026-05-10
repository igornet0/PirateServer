import React, { useEffect, useMemo, useState } from "react";
import { SecretFieldRow } from "./hostServiceSecretFields";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-3 py-2 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-amber-600/60 disabled:opacity-50";
const fieldClass =
  "w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-xs text-slate-100 placeholder:text-slate-600 focus:border-amber-600/40 focus:outline-none";
const labelClass = "mb-0.5 block text-[11px] font-medium text-slate-400";

function isParamService(id: string): id is "minio" | "meilisearch" | "postgresql" | "redis" {
  return id === "minio" || id === "meilisearch" || id === "postgresql" || id === "redis";
}

type Props = {
  serviceId: string;
  displayName: string;
  onClose: () => void;
  /** Keys are env names sent in POST body `env` (filtered on server by service id). */
  onConfirm: (env: Record<string, string>) => void;
  tr: (ru: string, en: string) => string;
};

/**
 * Shown before `POST /api/v1/host-services/:id/install` — optional parameters (MinIO, Meili, PG, Redis)
 * or an acknowledgement for other packages.
 */
export function HostServiceInstallDialog({ serviceId, displayName, onClose, onConfirm, tr }: Props) {
  const param = isParamService(serviceId);

  const [minioApi, setMinioApi] = useState("127.0.0.1:9000");
  const [minioConsole, setMinioConsole] = useState("127.0.0.1:9001");
  const [minioData, setMinioData] = useState("/var/lib/pirate/minio");
  const [minioUser, setMinioUser] = useState("minioadmin");
  const [minioPass, setMinioPass] = useState("");

  const [meiliHttp, setMeiliHttp] = useState("127.0.0.1:7700");
  const [meiliDb, setMeiliDb] = useState("/var/lib/pirate/meili/data");
  const [meiliVer, setMeiliVer] = useState("1.11.0");
  const [meiliKey, setMeiliKey] = useState("");

  const [pgListen, setPgListen] = useState("127.0.0.1");
  const [pgPort, setPgPort] = useState("5432");
  const [explorerUser, setExplorerUser] = useState("pirate_explorer");
  const [explorerDb, setExplorerDb] = useState("pirate_explorer");
  const [explorerPass, setExplorerPass] = useState("");
  const [explorerHost, setExplorerHost] = useState("127.0.0.1");
  const [explorerPort, setExplorerPort] = useState("5432");

  const [redisBind, setRedisBind] = useState("127.0.0.1");
  const [redisPort, setRedisPort] = useState("6379");
  const [redisAuthMode, setRedisAuthMode] = useState<"requirepass" | "acl">("requirepass");
  const [redisAclUser, setRedisAclUser] = useState("pirate_redis");
  const [redisPass, setRedisPass] = useState("");

  const ackText = useMemo(() => {
    const id = serviceId;
    if (id === "mysql" || id === "mongodb" || id === "mssql" || id === "clickhouse") {
      return tr(
        "Будет установлен пакет из репозитория дистрибутива с настройками по умолчанию. Смена портов и паролей — вручную на сервере после установки. Данные в каталоге данных СУБД могут быть утеряны при удалении пакетов с хоста.",
        "A distro package will be installed with default settings. Change ports/passwords on the host after install. Database files may be removed if you later uninstall the server packages from this host.",
      );
    }
    if (id === "node" || id === "python3") {
      return tr(
        "Среда для запуска проектов: будут поставлены пакеты по сценарию install-скрипта (см. server-stack).",
        "Runtimes for projects: install scripts from server-stack will add the usual OS packages.",
      );
    }
    if (id === "nginx") {
      return tr("Будет установлен nginx из репозитория. Vhost — на вкладке «nginx».", "OS nginx package. Edit vhost on the «nginx» tab.");
    }
    if (id === "cifs_utils") {
      return tr("Утилиты mount.cifs из apt.", "CIFS client utilities from apt.");
    }
    return tr(
      "Будет выполнён install-скрипт на сервере (root, sudo). Убедитесь, что это подходит для вашей среды.",
      "The host install script will run as root via sudo. Confirm this matches your environment.",
    );
  }, [serviceId, tr]);

  useEffect(() => {
    if (!param) return;
    if (serviceId === "minio") {
      setMinioApi("127.0.0.1:9000");
      setMinioConsole("127.0.0.1:9001");
      setMinioData("/var/lib/pirate/minio");
      setMinioUser("minioadmin");
      setMinioPass("");
    } else if (serviceId === "meilisearch") {
      setMeiliHttp("127.0.0.1:7700");
      setMeiliDb("/var/lib/pirate/meili/data");
      setMeiliVer("1.11.0");
      setMeiliKey("");
    } else if (serviceId === "postgresql") {
      setPgListen("127.0.0.1");
      setPgPort("5432");
      setExplorerUser("pirate_explorer");
      setExplorerDb("pirate_explorer");
      setExplorerPass("");
      setExplorerHost("127.0.0.1");
      setExplorerPort("5432");
    } else if (serviceId === "redis") {
      setRedisBind("127.0.0.1");
      setRedisPort("6379");
      setRedisAuthMode("requirepass");
      setRedisAclUser("pirate_redis");
      setRedisPass("");
    }
  }, [param, serviceId]);

  const submitParams = () => {
    if (serviceId === "minio") {
      const e: Record<string, string> = {
        PIRATE_MINIO_API_ADDR: minioApi.trim(),
        PIRATE_MINIO_CONSOLE_ADDR: minioConsole.trim(),
        PIRATE_MINIO_DATA_DIR: minioData.trim(),
        MINIO_ROOT_USER: minioUser.trim(),
      };
      if (minioPass.trim()) e.MINIO_ROOT_PASSWORD = minioPass.trim();
      onConfirm(e);
      return;
    }
    if (serviceId === "meilisearch") {
      const e: Record<string, string> = {
        PIRATE_MEILI_HTTP_ADDR: meiliHttp.trim(),
        PIRATE_MEILI_DB_PATH: meiliDb.trim(),
        PIRATE_MEILISEARCH_VERSION: meiliVer.trim(),
      };
      if (meiliKey.trim()) e.MEILI_MASTER_KEY = meiliKey.trim();
      onConfirm(e);
      return;
    }
    if (serviceId === "postgresql") {
      const e: Record<string, string> = {
        PIRATE_POSTGRESQL_LISTEN_ADDRESSES: pgListen.trim(),
        PIRATE_POSTGRESQL_PORT: pgPort.trim(),
        PIRATE_EXPLORER_DB_USER: explorerUser.trim(),
        PIRATE_EXPLORER_DB_NAME: explorerDb.trim(),
        PIRATE_EXPLORER_DB_HOST: explorerHost.trim(),
        PIRATE_EXPLORER_DB_PORT: explorerPort.trim(),
      };
      if (explorerPass.trim()) e.PIRATE_EXPLORER_DB_PASSWORD = explorerPass.trim();
      onConfirm(e);
      return;
    }
    if (serviceId === "redis") {
      const e: Record<string, string> = {
        PIRATE_REDIS_BIND: redisBind.trim(),
        PIRATE_REDIS_PORT: redisPort.trim(),
        PIRATE_REDIS_AUTH_MODE: redisAuthMode,
      };
      if (redisPass.trim()) e.PIRATE_REDIS_PASSWORD = redisPass.trim();
      if (redisAuthMode === "acl") {
        e.PIRATE_REDIS_ACL_USERNAME = redisAclUser.trim() || "pirate_redis";
      }
      onConfirm(e);
      return;
    }
  };

  const title = tr("Параметры установки", "Install options");

  return (
    <div className="fixed inset-0 z-modalNestedHigh flex items-center justify-center bg-black/60 p-4">
      <div
        className="max-h-[90vh] w-full max-w-lg overflow-y-auto rounded-xl border border-amber-900/30 bg-[#0a0a0c] p-4 shadow-xl"
        role="dialog"
        aria-labelledby="hs-install-title"
      >
        <h2 id="hs-install-title" className="text-sm font-semibold text-slate-100">
          {title}: {displayName}
        </h2>
        <p className="mt-0.5 font-mono text-[11px] text-amber-200/80">{serviceId}</p>

        {param && serviceId === "minio" ? (
          <div className="mt-4 space-y-3 text-xs">
            <p className="text-slate-500">
              {tr(
                "Адреса в формате host:port (по умолчанию loopback). Пустой пароль root — сгенерировать в /etc/pirate-minio.env.",
                "Addresses as host:port (loopback by default). Empty root password → generated in /etc/pirate-minio.env.",
              )}
            </p>
            <div>
              <label className={labelClass}>PIRATE_MINIO_API_ADDR</label>
              <input className={fieldClass} value={minioApi} onChange={(e) => setMinioApi(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>PIRATE_MINIO_CONSOLE_ADDR</label>
              <input className={fieldClass} value={minioConsole} onChange={(e) => setMinioConsole(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>PIRATE_MINIO_DATA_DIR</label>
              <input className={fieldClass} value={minioData} onChange={(e) => setMinioData(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>MINIO_ROOT_USER</label>
              <input className={fieldClass} value={minioUser} onChange={(e) => setMinioUser(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>MINIO_ROOT_PASSWORD {tr("(необязательно)", "(optional)")}</label>
              <SecretFieldRow
                value={minioPass}
                onChange={setMinioPass}
                tr={tr}
                inputClassName={`${fieldClass} min-w-0 flex-1`}
                placeholder="••••••••"
              />
            </div>
          </div>
        ) : null}

        {param && serviceId === "meilisearch" ? (
          <div className="mt-4 space-y-3 text-xs">
            <p className="text-slate-500">
              {tr(
                "Meilisearch слушает HTTP на loopback, данные в каталоге на диске. Пустой master key — сгенерировать в /etc/pirate-meilisearch.env.",
                "Meilisearch HTTP on loopback, data on disk. Empty master key → written to /etc/pirate-meilisearch.env.",
              )}
            </p>
            <div>
              <label className={labelClass}>PIRATE_MEILI_HTTP_ADDR</label>
              <input className={fieldClass} value={meiliHttp} onChange={(e) => setMeiliHttp(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>PIRATE_MEILI_DB_PATH</label>
              <input className={fieldClass} value={meiliDb} onChange={(e) => setMeiliDb(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>PIRATE_MEILISEARCH_VERSION</label>
              <input className={fieldClass} value={meiliVer} onChange={(e) => setMeiliVer(e.target.value)} />
            </div>
            <div>
              <label className={labelClass}>MEILI_MASTER_KEY {tr("(необязательно)", "(optional)")}</label>
              <SecretFieldRow
                value={meiliKey}
                onChange={setMeiliKey}
                tr={tr}
                inputClassName={`${fieldClass} min-w-0 flex-1`}
                placeholder="••••••••"
              />
            </div>
          </div>
        ) : null}

        {param && serviceId === "redis" ? (
          <div className="mt-4 space-y-3 text-xs">
            <p className="text-slate-500">
              {tr(
                "Bind и порт Redis, режим пароля. Пустой пароль — сгенерирует скрипт на сервере. ACL — отдельный пользователь, default отключается.",
                "Redis bind/port and auth mode. Empty password → generated on the host. ACL mode uses a dedicated user and disables the default user.",
              )}
            </p>
            <div>
              <label className={labelClass}>PIRATE_REDIS_BIND</label>
              <input className={fieldClass} value={redisBind} onChange={(e) => setRedisBind(e.target.value)} placeholder="127.0.0.1" />
            </div>
            <div>
              <label className={labelClass}>PIRATE_REDIS_PORT</label>
              <input className={fieldClass} value={redisPort} onChange={(e) => setRedisPort(e.target.value)} placeholder="6379" />
            </div>
            <div>
              <span className={labelClass}>{tr("Режим аутентификации", "Auth mode")}</span>
              <div className="mt-1 flex flex-wrap gap-3 text-slate-300">
                <label className="flex cursor-pointer items-center gap-2">
                  <input
                    type="radio"
                    name="redis-auth"
                    checked={redisAuthMode === "requirepass"}
                    onChange={() => setRedisAuthMode("requirepass")}
                    className="text-amber-600"
                  />
                  requirepass (default user)
                </label>
                <label className="flex cursor-pointer items-center gap-2">
                  <input
                    type="radio"
                    name="redis-auth"
                    checked={redisAuthMode === "acl"}
                    onChange={() => setRedisAuthMode("acl")}
                    className="text-amber-600"
                  />
                  ACL user
                </label>
              </div>
            </div>
            {redisAuthMode === "acl" ? (
              <div>
                <label className={labelClass}>PIRATE_REDIS_ACL_USERNAME</label>
                <input className={fieldClass} value={redisAclUser} onChange={(e) => setRedisAclUser(e.target.value)} />
              </div>
            ) : null}
            <div>
              <label className={labelClass}>PIRATE_REDIS_PASSWORD {tr("(необязательно)", "(optional)")}</label>
              <SecretFieldRow
                value={redisPass}
                onChange={setRedisPass}
                tr={tr}
                inputClassName={`${fieldClass} min-w-0 flex-1`}
                placeholder="••••••••"
              />
            </div>
          </div>
        ) : null}

        {param && serviceId === "postgresql" ? (
          <div className="mt-4 space-y-3 text-xs">
            <p className="text-slate-500">
              {tr(
                "listen_addresses и порт кластера PostgreSQL; отдельно хост/порт для строки POSTGRES_EXPLORER_URL и pg_hba. Пустой пароль — сгенерирует скрипт.",
                "Cluster listen_addresses and port; host/port for POSTGRES_EXPLORER_URL and pg_hba. Empty password → generated by the script.",
              )}
            </p>
            <div>
              <label className={labelClass}>PIRATE_POSTGRESQL_LISTEN_ADDRESSES</label>
              <input
                className={fieldClass}
                value={pgListen}
                onChange={(e) => setPgListen(e.target.value)}
                placeholder="127.0.0.1 или *"
              />
            </div>
            <div>
              <label className={labelClass}>PIRATE_POSTGRESQL_PORT</label>
              <input className={fieldClass} value={pgPort} onChange={(e) => setPgPort(e.target.value)} />
            </div>
            <div className="border-t border-white/10 pt-3">
              <p className="mb-2 text-[11px] font-medium text-slate-500">{tr("Explorer (UI)", "Explorer (UI)")}</p>
              <div>
                <label className={labelClass}>PIRATE_EXPLORER_DB_USER</label>
                <input className={fieldClass} value={explorerUser} onChange={(e) => setExplorerUser(e.target.value)} />
              </div>
              <div className="mt-2">
                <label className={labelClass}>PIRATE_EXPLORER_DB_NAME</label>
                <input className={fieldClass} value={explorerDb} onChange={(e) => setExplorerDb(e.target.value)} />
              </div>
              <div className="mt-2">
                <label className={labelClass}>PIRATE_EXPLORER_DB_PASSWORD {tr("(необязательно)", "(optional)")}</label>
                <SecretFieldRow
                  value={explorerPass}
                  onChange={setExplorerPass}
                  tr={tr}
                  inputClassName={`${fieldClass} min-w-0 flex-1`}
                  placeholder="••••••••"
                />
              </div>
              <div className="mt-2">
                <label className={labelClass}>PIRATE_EXPLORER_DB_HOST</label>
                <input className={fieldClass} value={explorerHost} onChange={(e) => setExplorerHost(e.target.value)} />
              </div>
              <div className="mt-2">
                <label className={labelClass}>PIRATE_EXPLORER_DB_PORT</label>
                <input className={fieldClass} value={explorerPort} onChange={(e) => setExplorerPort(e.target.value)} />
              </div>
            </div>
          </div>
        ) : null}

        {!param ? (
          <p className="mt-4 text-xs leading-relaxed text-slate-400">{ackText}</p>
        ) : null}

        <div className="mt-6 flex flex-wrap gap-2">
          <button
            type="button"
            className={`${btnSm} border border-amber-700/50 bg-amber-950/40 text-amber-100 hover:bg-amber-950/55`}
            onClick={() => (param ? submitParams() : onConfirm({}))}
          >
            {tr("Установить", "Install")}
          </button>
          <button type="button" className={`${btnSm} border border-white/10 bg-white/5 text-slate-300`} onClick={onClose}>
            {tr("Отмена", "Cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
