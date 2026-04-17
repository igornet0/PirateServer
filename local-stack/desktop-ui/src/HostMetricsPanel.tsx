import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  AlertCircle,
  ChevronDown,
  ChevronUp,
  Cpu,
  Flame,
  HardDrive,
  Loader2,
  Network,
  Server,
  Thermometer,
  X,
} from "lucide-react";
import React, { useCallback, useState } from "react";
import {
  HOST_STATS_DETAIL_KIND,
  type HostStatsSnapshot,
} from "./host-stats-types";
import { NetworkHostSeriesModal } from "./NetworkHostSeriesModal";
import { useI18n } from "./i18n";
import { ModalDialog } from "./ui/ModalDialog";

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${u[i]}`;
}

function formatRateBps(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n.toFixed(0)} B/s`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB/s`;
  return `${(n / 1024 / 1024).toFixed(2)} MiB/s`;
}

function sumNetRxTx(data: HostStatsSnapshot): { rx: number; tx: number } {
  let rx = 0;
  let tx = 0;
  for (const i of data.network_interfaces) {
    rx += i.rx_bytes_per_s;
    tx += i.tx_bytes_per_s;
  }
  return { rx, tx };
}

type StatusLevel = "ok" | "warn" | "crit";

function cpuStatus(pct: number): StatusLevel {
  if (pct >= 85) return "crit";
  if (pct >= 70) return "warn";
  return "ok";
}

function pctStatus(pct: number): StatusLevel {
  if (pct >= 85) return "crit";
  if (pct >= 70) return "warn";
  return "ok";
}

function tempStatus(c: number | null | undefined): StatusLevel {
  if (c == null || !Number.isFinite(c)) return "ok";
  if (c >= 90) return "crit";
  if (c >= 75) return "warn";
  return "ok";
}

function diskUsagePct(data: HostStatsSnapshot): number {
  const t = data.disk_total_bytes;
  if (!t) return 0;
  return ((t - data.disk_free_bytes) / t) * 100;
}

function memoryUsagePct(data: HostStatsSnapshot): number {
  const t = data.memory_total_bytes;
  if (!t) return 0;
  return (data.memory_used_bytes / t) * 100;
}

function statusRing(s: StatusLevel): string {
  if (s === "crit") return "ring-rose-500/50 border-rose-700/40 bg-rose-950/25";
  if (s === "warn") return "ring-red-600/35 border-red-800/35 bg-red-950/22";
  return "ring-emerald-500/30 border-white/10 bg-black/20";
}

type CardKind = "cpu" | "memory" | "disk" | "network" | "temp" | "processes";

function ProgressBarMini({ ratio }: { ratio: number }) {
  const w = Math.min(100, Math.max(0, ratio * 100));
  return (
    <div className="mt-3 h-2 w-full overflow-hidden rounded-full bg-black/30">
      <div
        className="h-full rounded-full bg-gradient-to-r from-red-800 via-orange-600 to-red-700"
        style={{ width: `${w}%` }}
      />
    </div>
  );
}

export function HostMetricsPanel({
  metrics,
  metricsLoading,
  metricsErr,
  useMockMetrics,
  onLoad,
  endpoint,
  /** HTTP base for control-api charts (`/api/v1/host-stats/series`). Not the gRPC URL. */
  seriesBaseUrl,
}: {
  metrics: HostStatsSnapshot | null;
  metricsLoading: boolean;
  metricsErr: string | null;
  useMockMetrics: boolean;
  onLoad: () => void;
  endpoint: string | null;
  seriesBaseUrl: string | null;
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
    const [metricsExpanded, setMetricsExpanded] = useState(false);
  const [detailOpen, setDetailOpen] = useState(false);
  const [detailTitle, setDetailTitle] = useState("");
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailErr, setDetailErr] = useState<string | null>(null);
  const [detailBody, setDetailBody] = useState<unknown>(null);

  const [netSeriesOpen, setNetSeriesOpen] = useState(false);
  const [netSeriesIface, setNetSeriesIface] = useState<string | null>(null);

  const openDetailGrpc = useCallback(async (title: string, kind: number) => {
    setDetailTitle(title);
    setDetailOpen(true);
    setDetailErr(null);
    setDetailBody(null);
    setDetailLoading(true);
    try {
      const raw = await invoke<string>("fetch_remote_host_stats_detail", {
        kind,
        top: 25,
        q: "",
        limit: 120,
      });
      setDetailBody(JSON.parse(raw) as unknown);
    } catch (e) {
      setDetailErr(String(e));
    } finally {
      setDetailLoading(false);
    }
  }, []);

  const openTempSnapshot = useCallback((m: HostStatsSnapshot) => {
    setDetailTitle(t("auto.HostMetricsPanel_tsx.1"));
    setDetailOpen(true);
    setDetailErr(null);
    setDetailBody({
      temperature_current_celsius: m.temperature_current_celsius,
      temperature_avg_celsius: m.temperature_avg_celsius,
      note:
        tr(
          "Снимок из GetHostStats. Исторические графики (CONTROL_API_HOST_STATS_SERIES) и live stream (CONTROL_API_HOST_STATS_STREAM) отдаются по HTTP control-api, не через этот gRPC клиент.",
          "Snapshot from GetHostStats. History charts (CONTROL_API_HOST_STATS_SERIES) and live stream (CONTROL_API_HOST_STATS_STREAM) are served by control-api HTTP, not this gRPC client.",
        ),
    });
    setDetailLoading(false);
  }, []);

  const onCardClick = useCallback(
    (k: CardKind, m: HostStatsSnapshot) => {
      if (k === "temp") {
        openTempSnapshot(m);
        return;
      }
      if (k === "network") {
        setNetSeriesIface(null);
        setNetSeriesOpen(true);
        return;
      }
      const map: Record<Exclude<CardKind, "temp" | "network">, number> = {
        cpu: HOST_STATS_DETAIL_KIND.CPU,
        memory: HOST_STATS_DETAIL_KIND.MEMORY,
        disk: HOST_STATS_DETAIL_KIND.DISK,
        processes: HOST_STATS_DETAIL_KIND.PROCESSES,
      };
      const titles: Record<CardKind, string> = {
        cpu: t("auto.HostMetricsPanel_tsx.2"),
        memory: t("auto.HostMetricsPanel_tsx.3"),
        disk: t("auto.HostMetricsPanel_tsx.4"),
        network: t("auto.HostMetricsPanel_tsx.5"),
        processes: t("auto.HostMetricsPanel_tsx.6"),
        temp: t("auto.HostMetricsPanel_tsx.7"),
      };
      void openDetailGrpc(titles[k], map[k as Exclude<CardKind, "temp" | "network">]);
    },
    [openDetailGrpc, openTempSnapshot],
  );

  const openNetworkDetailFromSeries = useCallback(() => {
    setNetSeriesOpen(false);
    void openDetailGrpc("Network detail", HOST_STATS_DETAIL_KIND.NETWORK);
  }, [openDetailGrpc]);

  const closeDetail = useCallback(() => {
    setDetailOpen(false);
    setDetailBody(null);
    setDetailErr(null);
  }, []);

  return (
    <>
      <section
        className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card"
        aria-labelledby="metrics-heading"
      >
        <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
          <h2 id="metrics-heading" className="text-lg font-semibold text-slate-100">
            {t("auto.HostMetricsPanel_tsx.8")}
          </h2>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={metricsLoading || !endpoint}
              onClick={() => onLoad()}
              className={`${btnBase} bg-gradient-to-r from-red-800 to-red-900 text-sm text-white shadow-md shadow-red-950/30 hover:brightness-110 disabled:opacity-40`}
            >
              {metricsLoading ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Activity className="h-4 w-4" />
              )}
              {t("auto.HostMetricsPanel_tsx.9")}
            </button>
            <button
              type="button"
              onClick={() => setMetricsExpanded((v) => !v)}
              className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
              aria-expanded={metricsExpanded}
              aria-controls="remote-host-metrics-body"
            >
              {metricsExpanded ? <ChevronUp className="h-4 w-4" /> : <ChevronDown className="h-4 w-4" />}
              {metricsExpanded ? t("auto.HostMetricsPanel_tsx.10") : t("auto.HostMetricsPanel_tsx.11")}
            </button>
          </div>
        </div>
        {metricsExpanded ? (
          <div id="remote-host-metrics-body">
            <p className="mb-3 text-xs text-slate-500">
              {t("auto.HostMetricsPanel_tsx.12")}
              <code className="text-orange-200/85">GetHostStats</code>; {t("auto.HostMetricsPanel_tsx.13")}
              <code className="text-orange-200/85">GetHostStatsDetail</code>.{" "}
              {t("auto.HostMetricsPanel_tsx.14")}
            </p>
            {useMockMetrics && metrics ? (
              <p className="mb-3 text-xs text-orange-300/90">
                {t("auto.HostMetricsPanel_tsx.15")}
              </p>
            ) : null}
            {metricsErr && !useMockMetrics ? (
              <p className="mb-3 flex items-center gap-2 text-sm text-rose-300">
                <AlertCircle className="h-4 w-4 shrink-0" />
                {metricsErr}
              </p>
            ) : null}

            {metricsLoading ? (
              <div className="grid gap-4 sm:grid-cols-2" aria-busy="true">
                {[1, 2, 3, 4, 5, 6].map((i) => (
                  <div
                    key={i}
                    className="relative h-36 animate-pulse rounded-2xl border border-white/10 bg-black/20"
                  />
                ))}
              </div>
            ) : metrics ? (
              <div className="space-y-6">
            <div
              className={`relative grid gap-3 sm:grid-cols-2 xl:grid-cols-3 ${useMockMetrics ? "opacity-[0.88]" : ""}`}
              aria-label={useMockMetrics ? tr("Демо-метрики (не реальные данные)", "Demo metrics (not live data)") : undefined}
            >
              {useMockMetrics ? (
                <span className="pointer-events-none absolute right-1 top-1 z-10 rounded border border-orange-500/35 bg-black/55 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-orange-200/95">
                  Mock
                </span>
              ) : null}
              <MetricCard
                kicker="CPU"
                metric={`${metrics.cpu_usage_percent.toFixed(1)}%`}
                sub={`load ${metrics.load_average_1m.toFixed(2)} · ${metrics.load_average_5m.toFixed(2)} · ${metrics.load_average_15m.toFixed(2)}`}
                status={cpuStatus(metrics.cpu_usage_percent)}
                showBar
                barRatio={Math.min(1, metrics.cpu_usage_percent / 100)}
                icon={<Cpu className="h-5 w-5" />}
                onClick={() => onCardClick("cpu", metrics)}
              />
              <MetricCard
                kicker={t("auto.HostMetricsPanel_tsx.16")}
                metric={formatBytes(metrics.memory_used_bytes)}
                sub={`${formatBytes(metrics.memory_total_bytes)} total · ${memoryUsagePct(metrics).toFixed(0)}%`}
                status={pctStatus(memoryUsagePct(metrics))}
                showBar
                barRatio={Math.min(1, memoryUsagePct(metrics) / 100)}
                icon={<Activity className="h-5 w-5" />}
                onClick={() => onCardClick("memory", metrics)}
              />
              <MetricCard
                kicker={t("auto.HostMetricsPanel_tsx.17")}
                metric={formatBytes(metrics.disk_free_bytes)}
                sub={`${metrics.disk_mount_path || "?"} · ${diskUsagePct(metrics).toFixed(0)}% used`}
                status={pctStatus(diskUsagePct(metrics))}
                showBar
                barRatio={Math.min(1, diskUsagePct(metrics) / 100)}
                icon={<HardDrive className="h-5 w-5" />}
                onClick={() => onCardClick("disk", metrics)}
              />
              <MetricCard
                kicker={t("auto.HostMetricsPanel_tsx.18")}
                metric={`↓ ${formatRateBps(sumNetRxTx(metrics).rx)} · ↑ ${formatRateBps(sumNetRxTx(metrics).tx)}`}
                sub={tr(`${metrics.network_interfaces.length} интерфейс(ов)`, `${metrics.network_interfaces.length} iface(s)`)}
                status="ok"
                showBar={false}
                barRatio={0}
                icon={<Network className="h-5 w-5" />}
                onClick={() => onCardClick("network", metrics)}
              />
              <MetricCard
                kicker={t("auto.HostMetricsPanel_tsx.19")}
                metric={
                  metrics.temperature_current_celsius != null
                    ? `${metrics.temperature_current_celsius.toFixed(1)} °C`
                    : metrics.temperature_avg_celsius != null
                      ? `~${metrics.temperature_avg_celsius.toFixed(1)} °C`
                      : "—"
                }
                sub={
                  metrics.temperature_current_celsius != null || metrics.temperature_avg_celsius != null
                    ? t("auto.HostMetricsPanel_tsx.20")
                    : t("auto.HostMetricsPanel_tsx.21")
                }
                status={tempStatus(metrics.temperature_current_celsius ?? metrics.temperature_avg_celsius)}
                showBar={false}
                barRatio={0}
                icon={
                  <>
                    <Thermometer className="h-5 w-5" />
                    <Flame className="h-4 w-4 text-orange-500 opacity-80" />
                  </>
                }
                onClick={() => onCardClick("temp", metrics)}
              />
              <MetricCard
                kicker={t("auto.HostMetricsPanel_tsx.22")}
                metric={String(metrics.process_count)}
                sub={t("auto.HostMetricsPanel_tsx.23")}
                status="ok"
                showBar={false}
                barRatio={0}
                icon={<Server className="h-5 w-5" />}
                onClick={() => onCardClick("processes", metrics)}
              />
            </div>

            {metrics.disk_mounts.length > 0 ? (
              <div>
                <h3 className="mb-2 text-sm font-semibold text-slate-300">{t("auto.HostMetricsPanel_tsx.24")}</h3>
                <div className="max-h-48 overflow-auto rounded-xl border border-white/10">
                  <table className="w-full text-left text-xs">
                    <thead className="sticky top-0 bg-black/40 text-slate-400">
                      <tr>
                        <th className="p-2">{t("auto.HostMetricsPanel_tsx.25")}</th>
                        <th className="p-2">{t("auto.HostMetricsPanel_tsx.26")}</th>
                        <th className="p-2">{t("auto.HostMetricsPanel_tsx.27")}</th>
                        <th className="p-2">{t("auto.HostMetricsPanel_tsx.28")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {metrics.disk_mounts.map((m) => {
                        const usedPct =
                          m.total_bytes > 0
                            ? (((m.total_bytes - m.free_bytes) / m.total_bytes) * 100).toFixed(1)
                            : "0";
                        return (
                          <tr key={m.path} className="border-t border-white/5">
                            <td className="p-2 font-mono text-slate-200">{m.path}</td>
                            <td className="p-2">{formatBytes(m.free_bytes)}</td>
                            <td className="p-2">{formatBytes(m.total_bytes)}</td>
                            <td className="p-2">{usedPct}%</td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : null}

            {metrics.network_interfaces.length > 0 ? (
              <div>
                <h3 className="mb-2 text-sm font-semibold text-slate-300">
              {t("auto.HostMetricsPanel_tsx.29")}{" "}
              <span className="font-normal text-slate-500">{t("auto.HostMetricsPanel_tsx.30")}</span>
            </h3>
                <div className="max-h-48 overflow-auto rounded-xl border border-white/10">
                  <table className="w-full text-left text-xs">
                    <thead className="sticky top-0 bg-black/40 text-slate-400">
                      <tr>
                        <th className="p-2">{t("auto.HostMetricsPanel_tsx.31")}</th>
                        <th className="p-2">RX/s</th>
                        <th className="p-2">TX/s</th>
                        <th className="p-2">{t("auto.HostMetricsPanel_tsx.32")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {metrics.network_interfaces.map((n) => (
                        <tr
                          key={n.name}
                          className="cursor-pointer border-t border-white/5 hover:bg-white/5"
                          role="button"
                          tabIndex={0}
                          onClick={() => {
                            setNetSeriesIface(n.name);
                            setNetSeriesOpen(true);
                          }}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                              e.preventDefault();
                              setNetSeriesIface(n.name);
                              setNetSeriesOpen(true);
                            }
                          }}
                        >
                          <td className="p-2 font-mono text-slate-200">{n.name}</td>
                          <td className="p-2">{formatRateBps(n.rx_bytes_per_s)}</td>
                          <td className="p-2">{formatRateBps(n.tx_bytes_per_s)}</td>
                          <td className="p-2">
                            {n.rx_errors} / {n.tx_errors}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : null}

            {metrics.log_tail.length > 0 ? (
              <div>
                <h3 className="mb-2 text-sm font-semibold text-slate-300">{t("auto.HostMetricsPanel_tsx.33")}</h3>
                <ul className="max-h-40 space-y-1 overflow-auto rounded-xl border border-white/10 bg-black/25 p-3 font-mono text-[11px] text-slate-300">
                  {metrics.log_tail.map((line, i) => (
                    <li key={`${line.ts_ms}-${i}`}>
                      <span className="text-orange-200/75">{line.level}</span> {line.message}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
              </div>
            ) : (
              <p className="text-sm text-slate-500">
                {t("auto.HostMetricsPanel_tsx.34")}<strong>{t("auto.HostMetricsPanel_tsx.35")}</strong>.
              </p>
            )}
          </div>
        ) : (
          <p className="text-sm text-slate-500">{t("auto.HostMetricsPanel_tsx.36")}</p>
        )}
      </section>

      <NetworkHostSeriesModal
        open={netSeriesOpen}
        onClose={() => setNetSeriesOpen(false)}
        baseUrl={seriesBaseUrl}
        ifaceLabel={netSeriesIface}
        onOpenDetail={openNetworkDetailFromSeries}
      />

      {detailOpen ? (
        <ModalDialog
          open
          zClassName="z-modal"
          onClose={closeDetail}
          panelClassName="w-full max-w-3xl max-h-[85vh] min-h-0"
          aria-labelledby="host-detail-title"
        >
          <div className="flex max-h-[85vh] w-full flex-col rounded-2xl border border-white/10 bg-surface p-0 shadow-2xl">
            <div className="flex items-center justify-between border-b border-white/10 px-5 py-3">
              <h3 id="host-detail-title" className="text-lg font-semibold text-slate-100">
                {detailTitle}
              </h3>
              <button
                type="button"
                data-modal-initial-focus
                onClick={closeDetail}
                className={`${btnBase} border border-white/10 bg-white/5 p-2`}
                aria-label={t("auto.HostMetricsPanel_tsx.37")}
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
              {detailLoading ? (
                <div className="flex items-center gap-2 text-slate-400">
                  <Loader2 className="h-5 w-5 animate-spin" />
                  {t("auto.HostMetricsPanel_tsx.38")}
                </div>
              ) : detailErr ? (
                <p className="text-sm text-rose-300">{detailErr}</p>
              ) : detailBody != null ? (
                <DetailRenderer data={detailBody} tr={tr} t={t} />
              ) : null}
            </div>
          </div>
        </ModalDialog>
      ) : null}
    </>
  );
}

function MetricCard({
  kicker,
  metric,
  sub,
  status,
  showBar,
  barRatio,
  icon,
  onClick,
}: {
  kicker: string;
  metric: string;
  sub: string;
  status: StatusLevel;
  showBar: boolean;
  barRatio: number;
  icon: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`w-full rounded-2xl border p-4 text-left transition hover:brightness-110 ${statusRing(status)}`}
    >
      <div className="flex items-center gap-2 text-xs font-medium uppercase tracking-wide text-slate-500">
        <span className="text-red-400">{icon}</span>
        {kicker}
      </div>
      <p className="mt-2 text-lg font-semibold tabular-nums text-slate-100">{metric}</p>
      <p className="mt-1 text-xs text-slate-500">{sub}</p>
      {showBar ? <ProgressBarMini ratio={barRatio} /> : null}
    </button>
  );
}

function DetailRenderer({
  data,
  tr,
  t,
}: {
  data: unknown;
  tr: (ru: string, en: string) => string;
  t: (key: string) => string;
}) {
  if (data == null || typeof data !== "object") {
    return <pre className="whitespace-pre-wrap break-all text-xs text-slate-300">{String(data)}</pre>;
  }
  const o = data as Record<string, unknown>;
  const kind = typeof o.kind === "string" ? o.kind : "";
  const inner = o.data;

  if (kind === "cpu" && inner && typeof inner === "object") {
    const d = inner as Record<string, unknown>;
    const top = Array.isArray(d.top_processes) ? d.top_processes : [];
    return (
      <div className="space-y-4 text-sm">
        {d.loadavg != null && typeof d.loadavg === "object" ? (
          <p className="text-slate-300">
            {t("auto.HostMetricsPanel_tsx.39")}:{" "}
            <code className="text-amber-200">
              {JSON.stringify(d.loadavg)}
            </code>
          </p>
        ) : null}
        {d.times != null ? (
          <p className="text-slate-300">
            {t("auto.HostMetricsPanel_tsx.40")}: <code className="text-amber-200">{JSON.stringify(d.times)}</code>
          </p>
        ) : null}
        {d.series_hint != null ? (
          <p className="text-xs text-slate-500">
            {t("auto.HostMetricsPanel_tsx.41")}:{" "}
            <code>{JSON.stringify(d.series_hint)}</code>
          </p>
        ) : null}
        {top.length > 0 ? (
          <table className="w-full text-xs">
            <thead className="text-slate-400">
              <tr>
                <th className="p-1 text-left">PID</th>
                <th className="p-1 text-left">{t("auto.HostMetricsPanel_tsx.42")}</th>
                <th className="p-1 text-right">CPU %</th>
              </tr>
            </thead>
            <tbody>
              {top.map((row: unknown, i: number) => {
                const r = row as Record<string, unknown>;
                return (
                  <tr key={i} className="border-t border-white/5">
                    <td className="p-1 font-mono">{String(r.pid ?? "")}</td>
                    <td className="p-1">{String(r.name ?? "")}</td>
                    <td className="p-1 text-right tabular-nums">
                      {Number(r.cpu_percent ?? 0).toFixed(1)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : null}
        <pre className="max-h-48 overflow-auto rounded-lg bg-black/30 p-3 text-[11px] text-slate-400">
          {JSON.stringify(inner, null, 2)}
        </pre>
      </div>
    );
  }

  if (kind === "memory" && inner && typeof inner === "object") {
    const d = inner as Record<string, unknown>;
    const top = Array.isArray(d.top_processes) ? d.top_processes : [];
    return (
      <div className="space-y-4 text-sm">
        {d.memory != null ? (
          <pre className="overflow-auto rounded-lg bg-black/30 p-3 text-xs text-slate-300">
            {JSON.stringify(d.memory, null, 2)}
          </pre>
        ) : null}
        {top.length > 0 ? (
          <table className="w-full text-xs">
            <thead className="text-slate-400">
              <tr>
                <th className="p-1 text-left">PID</th>
                <th className="p-1 text-left">{t("auto.HostMetricsPanel_tsx.43")}</th>
                <th className="p-1 text-right">RSS</th>
              </tr>
            </thead>
            <tbody>
              {top.map((row: unknown, i: number) => {
                const r = row as Record<string, unknown>;
                return (
                  <tr key={i} className="border-t border-white/5">
                    <td className="p-1 font-mono">{String(r.pid ?? "")}</td>
                    <td className="p-1">{String(r.name ?? "")}</td>
                    <td className="p-1 text-right">{formatBytes(Number(r.memory_bytes ?? 0))}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : null}
        <pre className="max-h-40 overflow-auto text-[11px] text-slate-500">
          {JSON.stringify(inner, null, 2)}
        </pre>
      </div>
    );
  }

  if (kind === "disk" && inner && typeof inner === "object") {
    const d = inner as Record<string, unknown>;
    const mounts = Array.isArray(d.mounts) ? d.mounts : [];
    const top = Array.isArray(d.top_processes) ? d.top_processes : [];
    return (
      <div className="space-y-4 text-sm">
        {mounts.length > 0 ? (
          <table className="w-full text-xs">
            <thead className="text-slate-400">
              <tr>
                <th className="p-1 text-left">{t("auto.HostMetricsPanel_tsx.44")}</th>
                <th className="p-1 text-right">{t("auto.HostMetricsPanel_tsx.45")}</th>
                <th className="p-1 text-right">{t("auto.HostMetricsPanel_tsx.46")}</th>
              </tr>
            </thead>
            <tbody>
              {mounts.map((row: unknown, i: number) => {
                const r = row as Record<string, unknown>;
                return (
                  <tr key={i} className="border-t border-white/5">
                    <td className="p-1 font-mono">{String(r.path ?? "")}</td>
                    <td className="p-1 text-right">{formatBytes(Number(r.free_bytes ?? 0))}</td>
                    <td className="p-1 text-right">{formatBytes(Number(r.total_bytes ?? 0))}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : null}
        {d.io != null ? (
          <p className="text-xs text-slate-400">I/O: {JSON.stringify(d.io)}</p>
        ) : null}
        {top.length > 0 ? (
          <table className="w-full text-xs">
            <thead className="text-slate-400">
              <tr>
                <th className="p-1 text-left">PID</th>
                <th className="p-1 text-left">{t("auto.HostMetricsPanel_tsx.47")}</th>
                <th className="p-1 text-right">{t("auto.HostMetricsPanel_tsx.48")}</th>
                <th className="p-1 text-right">{t("auto.HostMetricsPanel_tsx.49")}</th>
              </tr>
            </thead>
            <tbody>
              {top.map((row: unknown, i: number) => {
                const r = row as Record<string, unknown>;
                return (
                  <tr key={i} className="border-t border-white/5">
                    <td className="p-1 font-mono">{String(r.pid ?? "")}</td>
                    <td className="p-1">{String(r.name ?? "")}</td>
                    <td className="p-1 text-right">{formatBytes(Number(r.read_bytes ?? 0))}</td>
                    <td className="p-1 text-right">{formatBytes(Number(r.write_bytes ?? 0))}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : null}
        <pre className="max-h-40 overflow-auto text-[11px] text-slate-500">
          {JSON.stringify(inner, null, 2)}
        </pre>
      </div>
    );
  }

  if (kind === "network" && inner && typeof inner === "object") {
    const d = inner as Record<string, unknown>;
    const ifs = Array.isArray(d.interfaces) ? d.interfaces : [];
    return (
      <div className="space-y-3 text-sm">
        {typeof d.connections_note === "string" ? (
          <p className="text-xs text-slate-400">{d.connections_note}</p>
        ) : null}
        {ifs.length > 0 ? (
          <table className="w-full text-xs">
            <thead className="text-slate-400">
              <tr>
                <th className="p-1 text-left">{t("auto.HostMetricsPanel_tsx.50")}</th>
                <th className="p-1 text-right">RX/s</th>
                <th className="p-1 text-right">TX/s</th>
              </tr>
            </thead>
            <tbody>
              {ifs.map((row: unknown, i: number) => {
                const r = row as Record<string, unknown>;
                return (
                  <tr key={i} className="border-t border-white/5">
                    <td className="p-1 font-mono">{String(r.name ?? "")}</td>
                    <td className="p-1 text-right">{formatRateBps(Number(r.rx_bytes_per_s ?? 0))}</td>
                    <td className="p-1 text-right">{formatRateBps(Number(r.tx_bytes_per_s ?? 0))}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : null}
        <pre className="max-h-48 overflow-auto text-[11px] text-slate-500">
          {JSON.stringify(inner, null, 2)}
        </pre>
      </div>
    );
  }

  if (kind === "processes" && inner && typeof inner === "object") {
    const d = inner as Record<string, unknown>;
    const procs = Array.isArray(d.processes) ? d.processes : [];
    return (
      <div className="space-y-2 text-sm">
        {d.total != null ? (
          <p className="text-slate-400">
            {t("auto.HostMetricsPanel_tsx.51")}: <span className="text-slate-200">{String(d.total)}</span>
          </p>
        ) : null}
        {procs.length > 0 ? (
          <div className="max-h-64 overflow-auto">
            <table className="w-full text-xs">
              <thead className="sticky top-0 bg-surface text-slate-400">
                <tr>
                  <th className="p-1 text-left">PID</th>
                  <th className="p-1 text-left">{t("auto.HostMetricsPanel_tsx.52")}</th>
                  <th className="p-1 text-right">CPU%</th>
                  <th className="p-1 text-right">Mem</th>
                </tr>
              </thead>
              <tbody>
                {procs.map((row: unknown, i: number) => {
                  const r = row as Record<string, unknown>;
                  return (
                    <tr key={i} className="border-t border-white/5">
                      <td className="p-1 font-mono">{String(r.pid ?? "")}</td>
                      <td className="p-1">{String(r.name ?? "")}</td>
                      <td className="p-1 text-right tabular-nums">
                        {Number(r.cpu_percent ?? 0).toFixed(1)}
                      </td>
                      <td className="p-1 text-right">{formatBytes(Number(r.memory_bytes ?? 0))}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <pre className="whitespace-pre-wrap break-all text-xs text-slate-300">
      {JSON.stringify(data, null, 2)}
    </pre>
  );
}
