/**
 * Host software inventory (GET /api/v1/host-services) and install/remove via control-api.
 */
import { invoke } from "@tauri-apps/api/core";
import { Filter, Loader2, RefreshCw, Settings2 } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { useI18n } from "./i18n";
import { HostServiceInstallDialog } from "./HostServiceInstallDialog";
import { HostServiceRuntimeDialog } from "./HostServiceRuntimeDialog";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 disabled:pointer-events-none disabled:opacity-50";

type HostServiceRow = {
  id: string;
  display_name: string;
  category: string;
  installed: boolean;
  version?: string | null;
  running?: boolean | null;
  systemd_unit?: string | null;
  actions: string;
  notes?: string | null;
  /** Добавлено в UI, если в ответе API ещё нет строки (старый control-api на сервере). */
  synthetic?: boolean;
  /** MinIO / Meilisearch: правка `/etc/pirate-*.env` и перезапуск unit. */
  runtime_configurable?: boolean;
};

type HostServicesView = {
  services: HostServiceRow[];
  cifs_mounts: string[];
  dispatch_script_present: boolean;
};

/** Preferred column order in filter chips; unknown ids follow alphabetically. */
const CATEGORY_ORDER = ["runtime", "web", "database", "storage", "search", "tunnel", "mail"] as const;

function sortCategoryIds(ids: string[]): string[] {
  const rank = (id: string) => {
    const i = (CATEGORY_ORDER as readonly string[]).indexOf(id);
    return i === -1 ? 1000 + id.charCodeAt(0) : i;
  };
  return [...ids].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}

/**
 * If the server still runs an older `deploy-control` that omits new host-service ids, the UI
 * would hide MinIO/Meilisearch — show placeholder rows with an upgrade note until the stack is OTA/updated.
 */
function mergeWithUiServicePlaceholders(
  raw: HostServiceRow[] | undefined,
  t: (ru: string, en: string) => string,
): HostServiceRow[] {
  const list = raw ? [...raw] : [];
  const have = new Set(list.map((r) => r.id));
  if (!have.has("minio")) {
    list.push({
      id: "minio",
      display_name: "MinIO (S3)",
      category: "storage",
      installed: false,
      version: null,
      running: null,
      systemd_unit: null,
      actions: "none",
      notes: t(
        "В ответе API нет MinIO: на сервере, скорее всего, старая сборка control-api/deploy-control. Обновите server-stack (OTA или install.sh) — тогда появятся версия, статус и кнопка «Установить».",
        "MinIO is missing from the host-services response — the server is likely on an old control-api/deploy-control build. Update server-stack (OTA or install.sh) to get version, status, and Install.",
      ),
      synthetic: true,
      runtime_configurable: false,
    });
  }
  if (!have.has("meilisearch")) {
    list.push({
      id: "meilisearch",
      display_name: "Meilisearch",
      category: "search",
      installed: false,
      version: null,
      running: null,
      systemd_unit: null,
      actions: "none",
      notes: t(
        "В ответе API нет Meilisearch: на сервере, скорее всего, старая сборка. Обновите server-stack (OTA или install.sh).",
        "Meilisearch is missing from the host-services response — likely an old build. Update server-stack (OTA or install.sh).",
      ),
      synthetic: true,
      runtime_configurable: false,
    });
  }
  if (!have.has("stack_tun_api")) {
    list.push({
      id: "stack_tun_api",
      display_name: "Stack tunnel API",
      category: "tunnel",
      installed: false,
      version: null,
      running: null,
      systemd_unit: null,
      actions: "none",
      notes: t(
        "В ответе API нет stack-tun-api: обновите server-stack (control-api/deploy-control) на хосте.",
        "Stack tunnel API is missing from host-services — update server-stack (control-api/deploy-control) on the host.",
      ),
      synthetic: true,
      runtime_configurable: false,
    });
  }
  return list;
}

export function HostServicesPanel({ sessionOk }: { sessionOk: boolean }) {
  const { language } = useI18n();
  const tr = useCallback(
    (ru: string, en: string) => (language === "ru" ? ru : en),
    [language],
  );
  const [data, setData] = useState<HostServicesView | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  /** install | remove — for progress label only */
  const [busyKind, setBusyKind] = useState<"install" | "remove" | null>(null);
  const [out, setOut] = useState<string | null>(null);
  const [confirmRemoveId, setConfirmRemoveId] = useState<string | null>(null);
  /** "all" or backend `category` string (e.g. runtime, database). */
  const [categoryFilter, setCategoryFilter] = useState<string>("all");
  const [nameFilter, setNameFilter] = useState("");
  const [pendingInstall, setPendingInstall] = useState<{ id: string; displayName: string } | null>(null);
  const [runtimeEdit, setRuntimeEdit] = useState<{ id: string; displayName: string } | null>(null);

  const categoryLabel = (cat: string) => {
    const c = cat.toLowerCase();
    const m: Record<string, string> = {
      runtime: tr("Рантайм (Node, Python…)", "Runtime (Node, Python…)"),
      web: tr("Web (nginx)", "Web (nginx)"),
      database: tr("Базы данных", "Databases"),
      storage: tr("Хранилище (SMB, MinIO…)", "Storage (SMB, MinIO…)"),
      search: tr("Поиск (Meilisearch)", "Search (Meilisearch)"),
      tunnel: tr("Туннели (stack-tun-api)", "Tunnels (stack-tun-api)"),
      mail: tr("Почта", "Mail"),
    };
    return m[c] ?? cat;
  };

  const mergedServices = useMemo(
    () => mergeWithUiServicePlaceholders(data?.services, (ru, en) => (language === "ru" ? ru : en)),
    [data?.services, language],
  );

  const distinctCategories = useMemo(() => {
    const set = new Set<string>();
    for (const s of mergedServices) {
      if (s.category?.trim()) set.add(s.category.trim().toLowerCase());
    }
    return sortCategoryIds([...set]);
  }, [mergedServices]);

  const filteredServices = useMemo(() => {
    const q = nameFilter.trim().toLowerCase();
    return mergedServices.filter((row) => {
      if (categoryFilter !== "all" && row.category.toLowerCase() !== categoryFilter) return false;
      if (!q) return true;
      const blob = `${row.display_name} ${row.id} ${row.category}`.toLowerCase();
      return blob.includes(q);
    });
  }, [mergedServices, categoryFilter, nameFilter]);

  const load = useCallback(async () => {
    setLoading(true);
    setOut(null);
    try {
      const j = await invoke<string>("control_api_fetch_host_services_json");
      setData(JSON.parse(j) as HostServicesView);
    } catch (e) {
      setData(null);
      setOut(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (sessionOk) void load();
  }, [sessionOk, load]);

  useEffect(() => {
    if (categoryFilter === "all") return;
    if (distinctCategories.length && !distinctCategories.includes(categoryFilter)) {
      setCategoryFilter("all");
    }
  }, [distinctCategories, categoryFilter]);

  const runInstall = async (id: string, env: Record<string, string>) => {
    setPendingInstall(null);
    setBusyId(id);
    setBusyKind("install");
    setOut(null);
    const installEnvJson = JSON.stringify({ env });
    try {
      const r = await invoke<string>("control_api_host_service_install", {
        id,
        installEnvJson, // tauri: maps to `install_env_json` in Rust
      });
      setOut(r);
      toast.success(tr("Готово", "Done"), {
        description: r.length > 200 ? `${r.slice(0, 200)}…` : r,
      });
      await load();
    } catch (e) {
      const msg = String(e);
      setOut(msg);
      toast.error(tr("Ошибка установки", "Install failed"), { description: msg });
    } finally {
      setBusyId(null);
      setBusyKind(null);
    }
  };

  const runRemove = async (id: string) => {
    setConfirmRemoveId(null);
    setBusyId(id);
    setBusyKind("remove");
    setOut(null);
    try {
      const r = await invoke<string>("control_api_host_service_remove", { id });
      setOut(r);
      toast.success(tr("Готово", "Done"), {
        description: r.length > 200 ? `${r.slice(0, 200)}…` : r,
      });
      await load();
    } catch (e) {
      const msg = String(e);
      setOut(msg);
      toast.error(tr("Ошибка удаления", "Remove failed"), { description: msg });
    } finally {
      setBusyId(null);
      setBusyKind(null);
    }
  };

  if (!sessionOk) {
    return (
      <p className="text-sm text-slate-500">
        {tr("Сначала войдите на вкладке «Подключение».", "Sign in on the Connection tab first.")}
      </p>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          disabled={loading}
          onClick={() => void load()}
          className={`${btnSm} border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10`}
        >
          {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
          {tr("Обновить", "Refresh")}
        </button>
        {data && !data.dispatch_script_present ? (
          <span className="text-xs text-amber-200/90">
            {tr(
              "Скрипт pirate-host-service.sh не найден на сервере — обновите server-stack (install.sh).",
              "pirate-host-service.sh not found on server — update server-stack (install.sh).",
            )}
          </span>
        ) : null}
      </div>

      {busyId && busyKind ? (
        <div
          className="space-y-2 rounded-xl border border-amber-900/35 bg-amber-950/20 px-3 py-2.5"
          role="status"
          aria-live="polite"
          aria-busy="true"
        >
          <p className="text-xs text-amber-100/90">
            {busyKind === "install"
              ? tr("Установка сервиса", "Installing service")
              : tr("Удаление сервиса", "Removing service")}
            : <code className="ml-1 font-mono text-amber-200">{busyId}</code>
          </p>
          <div className="host-svc-progress-track" aria-hidden>
            <div className="host-svc-progress-bar" />
          </div>
          <p className="text-[10px] text-slate-500">
            {tr(
              "Ожидание ответа сервера (sudo-скрипт на хосте). Для крупных пакетов это может занять несколько минут.",
              "Waiting for the server (sudo script on the host). Large packages may take several minutes.",
            )}
          </p>
        </div>
      ) : null}

      {data ? (
        <div className="space-y-2 rounded-xl border border-white/10 bg-black/20 p-3">
          <div className="flex flex-wrap items-center gap-2 text-xs text-slate-500">
            <Filter className="h-3.5 w-3.5 shrink-0 text-slate-500" aria-hidden />
            <span className="font-medium text-slate-400">{tr("Фильтр", "Filter")}</span>
            <span className="text-slate-600">·</span>
            <span>
              {filteredServices.length === mergedServices.length
                ? tr("все сервисы", "all services")
                : tr("показано", "showing") + ` ${filteredServices.length} / ${mergedServices.length}`}
            </span>
          </div>
          <div className="flex flex-wrap gap-1.5">
            <button
              type="button"
              onClick={() => setCategoryFilter("all")}
              className={`rounded-lg border px-2.5 py-1 text-xs font-medium transition ${
                categoryFilter === "all"
                  ? "border-amber-600/50 bg-amber-950/40 text-amber-100"
                  : "border-white/10 bg-white/5 text-slate-400 hover:bg-white/10"
              }`}
            >
              {tr("Все", "All")}
            </button>
            {distinctCategories.map((c) => (
              <button
                key={c}
                type="button"
                onClick={() => setCategoryFilter(c)}
                className={`rounded-lg border px-2.5 py-1 text-xs font-medium transition ${
                  categoryFilter === c
                    ? "border-amber-600/50 bg-amber-950/40 text-amber-100"
                    : "border-white/10 bg-white/5 text-slate-400 hover:bg-white/10"
                }`}
              >
                {categoryLabel(c)}
              </button>
            ))}
          </div>
          <div>
            <label className="mb-1 block text-[10px] font-medium uppercase tracking-wide text-slate-500">
              {tr("Поиск по названию / id", "Search by name / id")}
            </label>
            <input
              type="search"
              value={nameFilter}
              onChange={(e) => setNameFilter(e.target.value)}
              placeholder={tr("redis, minio, stack_tun_api…", "redis, minio, stack_tun_api…")}
              className="w-full max-w-md rounded-lg border border-white/10 bg-black/35 px-3 py-1.5 font-mono text-xs text-slate-200 placeholder:text-slate-600 focus:border-amber-600/40 focus:outline-none"
            />
          </div>
        </div>
      ) : null}

      {out ? (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded-lg border border-white/10 bg-black/30 p-3 font-mono text-[11px] text-slate-300">
          {out}
        </pre>
      ) : null}

      {data?.cifs_mounts?.length ? (
        <div className="rounded-lg border border-white/10 bg-black/20 p-3 text-xs text-slate-400">
          <span className="font-semibold text-slate-300">{tr("CIFS монтирования:", "CIFS mounts:")}</span>{" "}
          {data.cifs_mounts.join(", ")}
        </div>
      ) : null}

      <div className="overflow-x-auto rounded-xl border border-white/10">
        <table className="w-full min-w-[640px] text-left text-xs">
          <thead className="border-b border-white/10 bg-black/30 text-slate-500">
            <tr>
              <th className="px-3 py-2 font-medium">{tr("Сервис", "Service")}</th>
              <th className="px-3 py-2 font-medium">{tr("Категория", "Category")}</th>
              <th className="px-3 py-2 font-medium">{tr("Версия", "Version")}</th>
              <th className="px-3 py-2 font-medium">{tr("Статус", "Status")}</th>
              <th className="px-3 py-2 font-medium">{tr("Действия", "Actions")}</th>
            </tr>
          </thead>
          <tbody>
            {filteredServices.length === 0 ? (
              <tr>
                <td colSpan={5} className="px-3 py-6 text-center text-slate-500">
                  {tr("Нет строк по текущему фильтру.", "No rows match the current filter.")}
                </td>
              </tr>
            ) : null}
            {filteredServices.map((row) => (
              <tr
                key={row.id}
                className={`border-b border-white/5 text-slate-300 ${
                  row.synthetic ? "bg-amber-950/15" : ""
                }`}
              >
                <td className="px-3 py-2">
                  <div className="font-medium text-slate-200">{row.display_name}</div>
                  <div className="font-mono text-[10px] text-slate-500">{row.id}</div>
                  {row.notes ? <p className="mt-1 text-[10px] leading-snug text-slate-500">{row.notes}</p> : null}
                </td>
                <td className="px-3 py-2 text-slate-400" title={row.category}>
                  <span className="capitalize">{categoryLabel(row.category)}</span>
                </td>
                <td className="px-3 py-2 font-mono text-[11px]">{row.version ?? "—"}</td>
                <td className="px-3 py-2">
                  {row.running === undefined || row.running === null
                    ? row.systemd_unit
                      ? "—"
                      : "—"
                    : row.running
                      ? tr("запущен", "running")
                      : tr("остановлен", "stopped")}
                  {row.systemd_unit ? (
                    <span className="ml-1 font-mono text-[10px] text-slate-500">({row.systemd_unit})</span>
                  ) : null}
                </td>
                <td className="px-3 py-2">
                  <div className="flex flex-wrap gap-1">
                    {row.runtime_configurable &&
                    row.installed &&
                    !row.synthetic &&
                    data?.dispatch_script_present ? (
                      <button
                        type="button"
                        disabled={busyId !== null}
                        onClick={() => setRuntimeEdit({ id: row.id, displayName: row.display_name })}
                        className={`${btnSm} border border-amber-800/40 bg-amber-950/25 text-amber-100`}
                        title={tr("Параметры и перезапуск", "Parameters and restart")}
                      >
                        <Settings2 className="h-3 w-3" />
                        {tr("Параметры", "Params")}
                      </button>
                    ) : null}
                    {row.actions === "install" && data?.dispatch_script_present ? (
                      <button
                        type="button"
                        disabled={busyId !== null}
                        onClick={() => setPendingInstall({ id: row.id, displayName: row.display_name })}
                        className={`${btnSm} border border-emerald-800/40 bg-emerald-950/30 text-emerald-100`}
                      >
                        {busyId === row.id ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
                        {row.id === "stack_tun_api"
                          ? tr("Включить", "Enable")
                          : tr("Установить", "Install")}
                      </button>
                    ) : null}
                    {row.actions === "remove" && data?.dispatch_script_present ? (
                      <button
                        type="button"
                        disabled={busyId !== null}
                        onClick={() => setConfirmRemoveId(row.id)}
                        className={`${btnSm} border border-red-800/40 bg-red-950/30 text-red-100`}
                      >
                        {busyId === row.id ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
                        {row.id === "stack_tun_api"
                          ? tr("Выключить", "Disable")
                          : tr("Удалить", "Remove")}
                      </button>
                    ) : null}
                    {row.actions === "none" ? (
                      <span className="text-slate-600">—</span>
                    ) : null}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {loading && !data ? (
        <div className="flex justify-center py-8 text-slate-500">
          <Loader2 className="h-6 w-6 animate-spin" />
        </div>
      ) : null}

      {pendingInstall ? (
        <HostServiceInstallDialog
          serviceId={pendingInstall.id}
          displayName={pendingInstall.displayName}
          onClose={() => setPendingInstall(null)}
          onConfirm={(env) => void runInstall(pendingInstall.id, env)}
          tr={tr}
        />
      ) : null}

      {runtimeEdit ? (
        <HostServiceRuntimeDialog
          serviceId={runtimeEdit.id}
          displayName={runtimeEdit.displayName}
          onClose={() => setRuntimeEdit(null)}
          onAfterChange={() => void load()}
          tr={tr}
        />
      ) : null}

      {confirmRemoveId ? (
        <div className="fixed inset-0 z-modalNestedHigh flex items-center justify-center bg-black/60 p-4">
          <div className="max-w-md rounded-xl border border-red-900/40 bg-[#120808] p-4 shadow-xl">
            <p className="text-sm text-slate-200">
              {confirmRemoveId === "stack_tun_api"
                ? tr(
                    "Выключить systemd-сервис stack-tun-api на хосте (оставит бинарь и unit на месте)?",
                    "Disable the stack-tun-api systemd unit on the host (binary and unit files stay installed)?",
                  )
                : tr(
                    "Удалить пакеты этого сервиса на хосте? Для баз данных это может уничтожить данные.",
                    "Remove this service’s packages on the host? For databases this may destroy data.",
                  )}
            </p>
            <p className="mt-2 font-mono text-xs text-amber-200/90">{confirmRemoveId}</p>
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                type="button"
                className={`${btnSm} border border-red-700/50 bg-red-950/40 text-red-100`}
                onClick={() => void runRemove(confirmRemoveId)}
              >
                {confirmRemoveId === "stack_tun_api"
                  ? tr("Выключить", "Disable")
                  : tr("Подтвердить удаление", "Confirm remove")}
              </button>
              <button type="button" className={`${btnSm} border border-white/10 bg-white/5`} onClick={() => setConfirmRemoveId(null)}>
                {tr("Отмена", "Cancel")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
