import uPlot from "uplot";
import {
  apiToken,
  fetchHostStats,
  fetchHostStatsDetail,
  fetchHostStatsSeries,
} from "../api/client.js";
import { HostStatsClient } from "../api/host-stats.js";
import type {
  CpuDetail,
  DiskDetail,
  HostStatsDetailKind,
  HostStatsView,
  MemoryDetail,
  NetworkDetail,
  ProcessesDetail,
  SeriesResponse,
} from "../api/types.js";
import { ApiRequestError } from "../api/types.js";
import { t } from "../i18n/index.js";
import type { MessageKey } from "../i18n/translations.js";
import {
  cpuStatus,
  diskUsagePct,
  filterMountsForDisplay,
  filterNetInterfaces,
  memoryUsagePct,
  pctStatus,
  tempStatus,
  type StatusLevel,
} from "./host-stats/helpers.js";
import { bindSortableTable } from "./host-stats/metric-table.js";

const client = new HostStatsClient();

/** uPlot draws to canvas — CSS variables are not resolved; use real colors. */
const U_PLOT_ACCENT = "#c4a84a";
const U_PLOT_RX = "#5d9e7a";
const U_PLOT_TX = "#6b9ec4";
const U_PLOT_AXIS = "#8a8580";

type CardKind = HostStatsDetailKind | "temp";

const SPARK_LEN = 48;

let pollTimer: ReturnType<typeof setInterval> | null = null;
let sseAbort: AbortController | null = null;
let mainUplot: InstanceType<typeof uPlot> | null = null;
let drillUplot: InstanceType<typeof uPlot> | null = null;
let chartsAvailable: boolean | null = null;
let activeCard: CardKind | null = null;
let chartToolbarBound = false;
let lastOverview: HostStatsView | null = null;

const cpuSpark: number[] = [];
const netSpark: number[] = [];

let ssePending: HostStatsView | null = null;
let sseRaf = 0;

let showAllNetIf = false;
let chartsTabInited = false;

let drillResizeObs: ResizeObserver | null = null;
let drillChartRefreshTimer: ReturnType<typeof setTimeout> | null = null;

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) {
    return t("status.hostServer.na");
  }
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  const digits = v >= 10 || i === 0 ? 0 : 1;
  return `${v.toFixed(digits)} ${units[i]}`;
}

function formatRateBps(n: number): string {
  if (!Number.isFinite(n) || n < 0) {
    return t("status.hostServer.na");
  }
  if (n < 1024) {
    return `${n.toFixed(0)} B/s`;
  }
  if (n < 1024 * 1024) {
    return `${(n / 1024).toFixed(1)} KiB/s`;
  }
  return `${(n / 1024 / 1024).toFixed(2)} MiB/s`;
}

function sumNetRxTx(data: HostStatsView): { rx: number; tx: number } {
  const ifs = data.network_interfaces ?? [];
  let rx = 0;
  let tx = 0;
  for (const i of ifs) {
    rx += i.rx_bytes_per_s;
    tx += i.tx_bytes_per_s;
  }
  return { rx, tx };
}

function pushSparkFromOverview(data: HostStatsView): void {
  cpuSpark.push(data.cpu_usage_percent);
  if (cpuSpark.length > SPARK_LEN) {
    cpuSpark.shift();
  }
  const { rx, tx } = sumNetRxTx(data);
  const sum = rx + tx;
  netSpark.push(sum);
  if (netSpark.length > SPARK_LEN) {
    netSpark.shift();
  }
}

function buildSparkPathD(values: number[], w = 100, h = 28): string {
  if (values.length < 2) {
    return "";
  }
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = max - min || 1e-9;
  const pts: string[] = [];
  values.forEach((v, i) => {
    const x = (i / (values.length - 1)) * w;
    const y = h - 2 - ((v - min) / range) * (h - 4);
    pts.push(`${x.toFixed(2)},${y.toFixed(2)}`);
  });
  return `M ${pts.join(" L ")}`;
}

function statusClass(s: StatusLevel): string {
  if (s === "crit") {
    return "host-stats-card--crit";
  }
  if (s === "warn") {
    return "host-stats-card--warn";
  }
  return "host-stats-card--ok";
}

type CardSpecFull = {
  kind: CardKind;
  kicker: string;
  metric: string;
  sub: string;
  barPct: number;
  showBar: boolean;
  showSpark: boolean;
  sparkD: "cpu" | "net" | "none";
  status: StatusLevel;
};

function cardSpecsFull(data: HostStatsView): CardSpecFull[] {
  const na = t("status.hostServer.na");
  const { rx, tx } = sumNetRxTx(data);
  const cur = data.temperature_current_celsius;
  const avg = data.temperature_avg_celsius;
  const tempStr =
    cur != null && Number.isFinite(cur)
      ? `${cur.toFixed(1)} °C`
      : avg != null && Number.isFinite(avg)
        ? `~${avg.toFixed(1)} °C`
        : na;

  const memPct = memoryUsagePct(data);
  const diskPct = diskUsagePct(data);
  const cpuS = cpuStatus(data.cpu_usage_percent);
  const memS = pctStatus(memPct);
  const diskS = pctStatus(diskPct);

  const netSub = t("status.hostServer.netCardSub");
  const netMetric = `↓ ${formatRateBps(rx)} · ↑ ${formatRateBps(tx)}`;
  const netSum = rx + tx;
  let netStatus: StatusLevel = "ok";
  if (netSum >= 80 * 1024 * 1024) {
    netStatus = "crit";
  } else if (netSum >= 25 * 1024 * 1024) {
    netStatus = "warn";
  }

  const tempS = tempStatus(cur ?? avg ?? null);

  return [
    {
      kind: "cpu",
      kicker: t("status.hostServer.cardCpu"),
      metric: `${data.cpu_usage_percent.toFixed(1)}%`,
      sub: `load ${data.load_average_1m.toFixed(2)} · ${data.load_average_5m.toFixed(2)} · ${data.load_average_15m.toFixed(2)}`,
      barPct: Math.min(100, data.cpu_usage_percent),
      showBar: true,
      showSpark: cpuSpark.length >= 2,
      sparkD: "cpu",
      status: cpuS,
    },
    {
      kind: "memory",
      kicker: t("status.hostServer.cardMemory"),
      metric: formatBytes(data.memory_used_bytes),
      sub: `${formatBytes(data.memory_total_bytes)} total · ${memPct.toFixed(0)}%`,
      barPct: Math.min(100, memPct),
      showBar: true,
      showSpark: false,
      sparkD: "none",
      status: memS,
    },
    {
      kind: "disk",
      kicker: t("status.hostServer.cardDisk"),
      metric: formatBytes(data.disk_free_bytes),
      sub: `${data.disk_mount_path} · ${diskPct.toFixed(0)}% used`,
      barPct: Math.min(100, diskPct),
      showBar: true,
      showSpark: false,
      sparkD: "none",
      status: diskS,
    },
    {
      kind: "network",
      kicker: t("status.hostServer.cardNetwork"),
      metric: netMetric,
      sub: netSub,
      barPct: 0,
      showBar: false,
      showSpark: netSpark.length >= 2,
      sparkD: "net",
      status: netStatus === "ok" && netSum === 0 ? "ok" : netStatus,
    },
    {
      kind: "temp",
      kicker: t("status.hostServer.cardTemp"),
      metric: tempStr,
      sub:
        cur != null || avg != null
          ? t("status.hostServer.tempCurrent")
          : t("status.hostServer.na"),
      barPct: 0,
      showBar: false,
      showSpark: false,
      sparkD: "none",
      status: tempS,
    },
    {
      kind: "processes",
      kicker: t("status.hostServer.cardProcesses"),
      metric: String(data.process_count),
      sub: t("status.hostServer.procTotalSub"),
      barPct: 0,
      showBar: false,
      showSpark: false,
      sparkD: "none",
      status: "ok",
    },
  ];
}

function patchCardsInPlace(container: HTMLElement, data: HostStatsView): boolean {
  const specs = cardSpecsFull(data);
  for (const s of specs) {
    if (!container.querySelector(`button.host-stats-card[data-kind="${s.kind}"]`)) {
      return false;
    }
  }
  for (const s of specs) {
    const btn = container.querySelector(
      `button.host-stats-card[data-kind="${s.kind}"]`,
    ) as HTMLButtonElement | null;
    if (!btn) {
      return false;
    }
    btn.classList.remove("host-stats-card--ok", "host-stats-card--warn", "host-stats-card--crit");
    btn.classList.add(statusClass(s.status));
    const metric = btn.querySelector(".host-stats-card-metric");
    const sub = btn.querySelector(".host-stats-card-sub");
    const bar = btn.querySelector(".host-stats-card-bar-inner") as HTMLElement | null;
    const sparkPathEl = btn.querySelector(".host-stats-card-spark path") as SVGPathElement | null;
    if (s.kind === "network") {
      const { rx: r, tx: txv } = sumNetRxTx(data);
      if (metric) {
        metric.textContent = `↓ ${formatRateBps(r)} · ↑ ${formatRateBps(txv)}`;
      }
      if (sub) {
        sub.textContent = t("status.hostServer.netCardSub");
      }
    } else {
      if (metric) {
        metric.textContent = s.metric;
      }
      if (sub) {
        sub.textContent = s.sub;
      }
    }
    if (bar && s.showBar) {
      bar.style.width = `${s.barPct}%`;
      bar.parentElement!.hidden = false;
    } else if (bar?.parentElement) {
      bar.parentElement.hidden = !s.showBar;
    }
    const sparkSvg = btn.querySelector(".host-stats-card-spark") as SVGElement | null;
    if (sparkSvg) {
      const show =
        s.sparkD === "cpu"
          ? cpuSpark.length >= 2
          : s.sparkD === "net"
            ? netSpark.length >= 2
            : false;
      (sparkSvg as unknown as HTMLElement).hidden = !show;
      if (show && sparkPathEl) {
        const vals = s.sparkD === "cpu" ? cpuSpark : netSpark;
        sparkPathEl.setAttribute("d", buildSparkPathD(vals));
      }
    }
    btn.classList.toggle("is-active", activeCard === s.kind);
  }
  return true;
}

function renderCardsFull(container: HTMLElement, data: HostStatsView): void {
  container.textContent = "";
  const specs = cardSpecsFull(data);
  for (const s of specs) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `host-stats-card ${statusClass(s.status)}`;
    btn.dataset.kind = s.kind;
    if (activeCard === s.kind) {
      btn.classList.add("is-active");
    }
    const kicker = document.createElement("span");
    kicker.className = "host-stats-card-kicker";
    kicker.textContent = s.kicker;
    const metric = document.createElement("span");
    metric.className = "host-stats-card-metric";
    metric.textContent = s.metric;
    const sub = document.createElement("span");
    sub.className = "host-stats-card-sub";
    sub.textContent = s.sub;
    btn.appendChild(kicker);
    btn.appendChild(metric);
    btn.appendChild(sub);
    const sparkVals = s.sparkD === "cpu" ? cpuSpark : s.sparkD === "net" ? netSpark : [];
    const showSpark = sparkVals.length >= 2;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", "host-stats-card-spark");
    svg.setAttribute("viewBox", "0 0 100 28");
    svg.setAttribute("preserveAspectRatio", "none");
    (svg as unknown as HTMLElement).hidden = !showSpark;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    if (showSpark) {
      path.setAttribute("d", buildSparkPathD(sparkVals));
    }
    svg.appendChild(path);
    btn.appendChild(svg);
    const barWrap = document.createElement("div");
    barWrap.className = "host-stats-card-bar-wrap";
    barWrap.hidden = !s.showBar;
    const barInner = document.createElement("div");
    barInner.className = "host-stats-card-bar-inner";
    barInner.style.width = `${s.barPct}%`;
    barWrap.appendChild(barInner);
    btn.appendChild(barWrap);
    container.appendChild(btn);
  }
}

function renderStorageTab(container: HTMLElement, data: HostStatsView): void {
  container.textContent = "";
  const mounts = data.disk_mounts ?? [];
  if (mounts.length === 0) {
    return;
  }
  const { primary, other } = filterMountsForDisplay(mounts, data.disk_mount_path);
  const h = document.createElement("h3");
  h.textContent = t("status.hostServer.mountsHeading");
  h.style.margin = "0 0 0.35rem";
  h.style.fontSize = "0.82rem";
  container.appendChild(h);
  const wrap = document.createElement("div");
  wrap.className = "host-stats-extra-table-wrap";
  const table = document.createElement("table");
  table.className = "host-stats-extra";
  const thead = document.createElement("thead");
  thead.innerHTML = `<tr>
    <th data-sort-key="0">${escapeHtml(t("status.hostServer.diskMount"))}</th>
    <th data-sort-key="1">Free</th>
    <th data-sort-key="2">Total</th>
    <th data-sort-key="3">Used %</th>
  </tr>`;
  table.appendChild(thead);
  const tb = document.createElement("tbody");
  for (const m of primary) {
    const tr = document.createElement("tr");
    const usedPct =
      m.total_bytes > 0
        ? (((m.total_bytes - m.free_bytes) / m.total_bytes) * 100).toFixed(1)
        : "0";
    tr.innerHTML = `<td>${escapeHtml(m.path)}</td><td>${escapeHtml(formatBytes(m.free_bytes))}</td><td>${escapeHtml(formatBytes(m.total_bytes))}</td><td>${usedPct}%</td>`;
    tb.appendChild(tr);
  }
  table.appendChild(tb);
  wrap.appendChild(table);
  container.appendChild(wrap);
  bindSortableTable(table);

  if (other.length > 0) {
    const det = document.createElement("details");
    det.style.marginTop = "0.5rem";
    const summ = document.createElement("summary");
    summ.textContent = t("status.hostServer.otherMounts", { count: String(other.length) });
    det.appendChild(summ);
    const t2 = document.createElement("table");
    t2.className = "host-stats-extra";
    t2.style.marginTop = "0.35rem";
    const thead2 = document.createElement("thead");
    thead2.innerHTML = `<tr><th>Mount</th><th>Free</th><th>Total</th></tr>`;
    t2.appendChild(thead2);
    const tb2 = document.createElement("tbody");
    for (const m of other) {
      const tr = document.createElement("tr");
      tr.innerHTML = `<td>${escapeHtml(m.path)}</td><td>${escapeHtml(formatBytes(m.free_bytes))}</td><td>${escapeHtml(formatBytes(m.total_bytes))}</td>`;
      tb2.appendChild(tr);
    }
    t2.appendChild(tb2);
    det.appendChild(t2);
    container.appendChild(det);
  }
}

function renderNetworkTab(container: HTMLElement, data: HostStatsView): void {
  container.textContent = "";
  const ifs = data.network_interfaces ?? [];
  if (ifs.length === 0) {
    return;
  }
  const row = document.createElement("div");
  row.className = "host-drill-toolbar-row";
  const lab = document.createElement("label");
  lab.style.display = "inline-flex";
  lab.style.alignItems = "center";
  lab.style.gap = "0.35rem";
  lab.style.fontSize = "0.78rem";
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.id = "host-server-show-all-if";
  cb.checked = showAllNetIf;
  cb.addEventListener("change", () => {
    showAllNetIf = cb.checked;
    if (lastOverview) {
      renderNetworkTab(container, lastOverview);
    }
  });
  const sp = document.createElement("span");
  sp.textContent = t("status.hostServer.showAllInterfaces");
  lab.appendChild(cb);
  lab.appendChild(sp);
  row.appendChild(lab);
  container.appendChild(row);

  const { visible, hiddenCount } = filterNetInterfaces(ifs, showAllNetIf);
  const h = document.createElement("h3");
  h.textContent = t("status.hostServer.networkHeading");
  h.style.margin = "0.35rem 0";
  h.style.fontSize = "0.82rem";
  container.appendChild(h);
  if (!showAllNetIf && hiddenCount > 0) {
    const p = document.createElement("p");
    p.className = "muted";
    p.style.fontSize = "0.72rem";
    p.style.margin = "0 0 0.35rem";
    p.textContent = `+${hiddenCount} hidden (virtual / loopback)`;
    container.appendChild(p);
  }
  const table = document.createElement("table");
  table.className = "host-stats-extra";
  const thead = document.createElement("thead");
  thead.innerHTML = `<tr>
    <th data-sort-key="0">IF</th>
    <th data-sort-key="1">RX/s</th>
    <th data-sort-key="2">TX/s</th>
    <th data-sort-key="3">err</th>
  </tr>`;
  table.appendChild(thead);
  const tb = document.createElement("tbody");
  for (const i of visible) {
    const tr = document.createElement("tr");
    tr.innerHTML = `<td>${escapeHtml(i.name)}</td><td>${escapeHtml(formatRateBps(i.rx_bytes_per_s))}</td><td>${escapeHtml(formatRateBps(i.tx_bytes_per_s))}</td><td>${i.rx_errors} / ${i.tx_errors}</td>`;
    tb.appendChild(tr);
  }
  table.appendChild(tb);
  container.appendChild(table);
  bindSortableTable(table);
}

function renderLogsTab(container: HTMLElement, data: HostStatsView): void {
  container.textContent = "";
  const logs = data.log_tail ?? [];
  if (logs.length === 0) {
    return;
  }
  const h = document.createElement("h3");
  h.textContent = t("status.hostServer.logsHeading");
  h.style.margin = "0 0 0.35rem";
  h.style.fontSize = "0.82rem";
  container.appendChild(h);
  const pre = document.createElement("pre");
  pre.className = "host-log-tail";
  pre.style.maxHeight = "12rem";
  pre.style.overflow = "auto";
  pre.textContent = logs.map((l) => l.message).join("\n");
  container.appendChild(pre);
}

function renderSecondaryTabs(data: HostStatsView): void {
  const st = document.getElementById("host-server-tab-storage");
  const nt = document.getElementById("host-server-tab-network");
  const lt = document.getElementById("host-server-tab-logs");
  if (st) {
    renderStorageTab(st, data);
  }
  if (nt) {
    renderNetworkTab(nt, data);
  }
  if (lt) {
    renderLogsTab(lt, data);
  }
}

function destroyMainUplot(): void {
  if (mainUplot) {
    mainUplot.destroy();
    mainUplot = null;
  }
}

function destroyDrillUplot(): void {
  if (drillUplot) {
    drillUplot.destroy();
    drillUplot = null;
  }
  if (drillResizeObs) {
    drillResizeObs.disconnect();
    drillResizeObs = null;
  }
  if (drillChartRefreshTimer) {
    clearTimeout(drillChartRefreshTimer);
    drillChartRefreshTimer = null;
  }
}

function uplotTheme(): Pick<uPlot.Options, "axes" | "series"> {
  return {
    series: [{}, { stroke: U_PLOT_ACCENT, width: 1.5 }],
    axes: [{ stroke: U_PLOT_AXIS }, { stroke: U_PLOT_AXIS }],
  };
}

/** Width for drill charts: container is often 0px until the modal finishes layout. */
function drillPlotWidth(container: HTMLElement): number {
  let w = container.clientWidth;
  if (w > 64) {
    return w;
  }
  const body = container.closest(".host-server-drill-body");
  if (body instanceof HTMLElement) {
    const bw = body.clientWidth - 24;
    if (bw > 64) {
      return bw;
    }
  }
  const dlg = container.closest(".host-server-drill-dialog, .server-info-dialog");
  if (dlg instanceof HTMLElement) {
    const dw = dlg.clientWidth - 40;
    if (dw > 64) {
      return dw;
    }
  }
  return Math.min(720, Math.max(320, window.innerWidth - 96));
}

function scheduleDrillLayoutResize(container: HTMLElement): void {
  const fit = (): void => {
    if (!drillUplot) {
      return;
    }
    const w = drillPlotWidth(container);
    if (w > 64) {
      drillUplot.setSize({ width: w, height: drillUplot.height });
    }
  };
  requestAnimationFrame(() => {
    requestAnimationFrame(fit);
  });
}

function alignNetSeries(
  rx: SeriesResponse,
  tx: SeriesResponse,
): { xs: number[]; yRx: number[]; yTx: number[] } {
  const a = rx.points;
  const b = tx.points;
  if (a.length > 0 && a.length === b.length) {
    let pairwise = true;
    for (let i = 0; i < a.length; i++) {
      if (a[i].ts_ms !== b[i].ts_ms) {
        pairwise = false;
        break;
      }
    }
    if (pairwise) {
      return {
        xs: a.map((p) => p.ts_ms / 1000),
        yRx: a.map((p) => p.value),
        yTx: b.map((p) => p.value),
      };
    }
  }
  const map = new Map<number, { rx?: number; tx?: number }>();
  for (const p of a) {
    const e = map.get(p.ts_ms) ?? {};
    e.rx = p.value;
    map.set(p.ts_ms, e);
  }
  for (const p of b) {
    const e = map.get(p.ts_ms) ?? {};
    e.tx = p.value;
    map.set(p.ts_ms, e);
  }
  const keys = Array.from(map.keys()).sort((x, y) => x - y);
  const xs: number[] = [];
  const yRx: number[] = [];
  const yTx: number[] = [];
  for (const k of keys) {
    const v = map.get(k)!;
    if (typeof v.rx === "number" && typeof v.tx === "number") {
      xs.push(k / 1000);
      yRx.push(v.rx);
      yTx.push(v.tx);
    }
  }
  return { xs, yRx, yTx };
}

/** uPlot requires ascending unique x; API order is usually fine but sort defensively. */
function prepareSeriesForChart(s: SeriesResponse): SeriesResponse {
  if (s.points.length <= 1) {
    return s;
  }
  const pts = [...s.points].sort((a, b) => a.ts_ms - b.ts_ms);
  return { ...s, points: pts };
}

function drawMainChart(container: HTMLElement, series: SeriesResponse): void {
  destroyMainUplot();
  const ser = prepareSeriesForChart(series);
  const xs = ser.points.map((p) => p.ts_ms / 1000);
  const ys = ser.points.map((p) => p.value);
  if (xs.length === 0) {
    container.textContent = t("status.hostServer.na");
    return;
  }
  const opts: uPlot.Options = {
    width: container.clientWidth || 400,
    height: 160,
    scales: { x: { time: true } },
    ...uplotTheme(),
  };
  mainUplot = new uPlot(opts, [xs, ys], container);
}

function drawDrillSingle(
  container: HTMLElement,
  series: SeriesResponse,
  mode: "cpu" | "memory" = "cpu",
): void {
  destroyDrillUplot();
  const ser = prepareSeriesForChart(series);
  const xs = ser.points.map((p) => p.ts_ms / 1000);
  const ys = ser.points.map((p) => p.value);
  if (xs.length === 0) {
    container.textContent = t("status.hostServer.na");
    return;
  }
  const yAxis: uPlot.Axis =
    mode === "memory"
      ? {
          stroke: U_PLOT_AXIS,
          values: (u, splits) => splits.map((v) => formatBytes(v)),
        }
      : { stroke: U_PLOT_AXIS };
  const opts: uPlot.Options = {
    width: drillPlotWidth(container),
    height: 180,
    scales: { x: { time: true } },
    series: [{}, { stroke: U_PLOT_ACCENT, width: 1.5 }],
    axes: [{ stroke: U_PLOT_AXIS }, yAxis],
  };
  drillUplot = new uPlot(opts, [xs, ys], container);
  attachDrillResize(container);
  scheduleDrillLayoutResize(container);
}

function drawDrillDualNet(
  container: HTMLElement,
  rx: SeriesResponse,
  tx: SeriesResponse,
): void {
  destroyDrillUplot();
  const { xs, yRx, yTx } = alignNetSeries(
    prepareSeriesForChart(rx),
    prepareSeriesForChart(tx),
  );
  if (xs.length === 0) {
    container.textContent = t("status.hostServer.na");
    return;
  }
  const opts: uPlot.Options = {
    width: drillPlotWidth(container),
    height: 180,
    scales: { x: { time: true } },
    series: [
      {},
      { stroke: U_PLOT_RX, width: 1.5 },
      { stroke: U_PLOT_TX, width: 1.5 },
    ],
    axes: [{ stroke: U_PLOT_AXIS }, { stroke: U_PLOT_AXIS }],
  };
  drillUplot = new uPlot(opts, [xs, yRx, yTx], container);
  attachDrillResize(container);
  scheduleDrillLayoutResize(container);
}

function attachDrillResize(container: HTMLElement): void {
  if (drillResizeObs) {
    drillResizeObs.disconnect();
  }
  drillResizeObs = new ResizeObserver(() => {
    if (!drillUplot) {
      return;
    }
    const w = drillPlotWidth(container);
    if (w > 64) {
      drillUplot.setSize({ width: w, height: drillUplot.height });
    }
  });
  drillResizeObs.observe(container);
}

function scheduleDrillChartRefresh(
  fn: () => void,
  delayMs = 45_000,
): void {
  if (drillChartRefreshTimer) {
    clearTimeout(drillChartRefreshTimer);
  }
  drillChartRefreshTimer = setTimeout(fn, delayMs);
}

async function openDrillModal(kind: CardKind, overview: HostStatsView): Promise<void> {
  const dlg = document.getElementById(
    "dialog-host-server-drill",
  ) as HTMLDialogElement | null;
  const titleEl = document.getElementById("host-server-drill-title");
  const body = document.getElementById("host-server-drill-body");
  if (!dlg || !titleEl || !body) {
    return;
  }
  destroyDrillUplot();
  body.textContent = "";
  const na = t("status.hostServer.na");

  if (kind === "temp") {
    titleEl.textContent = t("status.hostServer.detailTemp");
    const cur = overview.temperature_current_celsius;
    const avg = overview.temperature_avg_celsius;
    body.innerHTML = `<p>${escapeHtml(t("status.hostServer.tempDetailBody"))}</p>
      <p>${escapeHtml(t("status.hostServer.tempCurrent"))}: ${cur != null && Number.isFinite(cur) ? `${cur.toFixed(1)} °C` : na}</p>
      <p>${escapeHtml(t("status.hostServer.tempAvg"))}: ${avg != null && Number.isFinite(avg) ? `${avg.toFixed(1)} °C` : na}</p>`;
    dlg.showModal();
    return;
  }

  const titles: Record<HostStatsDetailKind, string> = {
    cpu: t("status.hostServer.detailCpu"),
    memory: t("status.hostServer.detailMemory"),
    disk: t("status.hostServer.detailDisk"),
    network: t("status.hostServer.detailNetwork"),
    processes: t("status.hostServer.detailProcesses"),
  };
  titleEl.textContent = titles[kind as HostStatsDetailKind] ?? "—";
  body.innerHTML = `<p class="host-stats-loading">${escapeHtml(t("status.hostServer.loading"))}</p>`;
  dlg.showModal();

  try {
    if (kind === "cpu") {
      const wrap = document.createElement("div");
      const tool = document.createElement("div");
      tool.className = "host-drill-toolbar-row";
      tool.innerHTML = `<label>${escapeHtml(t("status.hostServer.chartRange"))}
        <select id="drill-range-cpu" class="host-drill-limit">
          <option value="15m">15m</option>
          <option value="1h" selected>1h</option>
          <option value="24h">24h</option>
        </select></label>
        <span>${escapeHtml(t("status.hostServer.loadAvgExplain"))}</span>`;
      const chartWrap = document.createElement("div");
      chartWrap.className = "host-drill-chart-wrap";
      const uroot = document.createElement("div");
      uroot.className = "host-drill-uplot";
      chartWrap.appendChild(uroot);
      wrap.appendChild(tool);
      wrap.appendChild(chartWrap);
      body.textContent = "";
      body.appendChild(wrap);

      const rangeSel = tool.querySelector("#drill-range-cpu") as HTMLSelectElement;
      const loadChart = async () => {
        const ser = await fetchHostStatsSeries("cpu", rangeSel.value);
        uroot.textContent = "";
        drawDrillSingle(uroot, ser, "cpu");
      };
      rangeSel.addEventListener("change", () => void loadChart());
      await loadChart();
      scheduleDrillChartRefresh(() => void loadChart());

      const d: CpuDetail = await fetchHostStatsDetail("cpu", { top: 20 });
      const loadP = document.createElement("p");
      loadP.style.fontSize = "0.82rem";
      loadP.appendChild(
        document.createTextNode(
          `${t("status.hostServer.load")}: ${d.loadavg.m1.toFixed(2)} / ${d.loadavg.m5.toFixed(2)} / ${d.loadavg.m15.toFixed(2)} `,
        ),
      );
      const help = document.createElement("abbr");
      help.className = "load-avg-help";
      help.title = t("status.hostServer.loadAvgExplain");
      help.textContent = "?";
      loadP.appendChild(help);
      body.appendChild(loadP);
      const tbl = document.createElement("table");
      tbl.className = "host-stats-detail";
      const thead = document.createElement("thead");
      thead.innerHTML = `<tr><th data-sort-key="0">PID</th><th data-sort-key="1">Name</th><th data-sort-key="2">CPU %</th></tr>`;
      tbl.appendChild(thead);
      const tb = document.createElement("tbody");
      for (const p of d.top_processes) {
        const tr = document.createElement("tr");
        if (p.cpu_percent >= 25) {
          tr.classList.add("host-stats-row--high");
        }
        tr.innerHTML = `<td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${p.cpu_percent.toFixed(1)}%</td>`;
        tb.appendChild(tr);
      }
      tbl.appendChild(tb);
      body.appendChild(tbl);
      bindSortableTable(tbl);
    } else if (kind === "memory") {
      const wrap = document.createElement("div");
      const tool = document.createElement("div");
      tool.className = "host-drill-toolbar-row";
      tool.innerHTML = `<label>${escapeHtml(t("status.hostServer.chartRange"))}
        <select id="drill-range-mem" class="host-drill-limit">
          <option value="15m">15m</option>
          <option value="1h" selected>1h</option>
          <option value="24h">24h</option>
        </select></label>`;
      const chartWrap = document.createElement("div");
      chartWrap.className = "host-drill-chart-wrap";
      const uroot = document.createElement("div");
      uroot.className = "host-drill-uplot";
      chartWrap.appendChild(uroot);
      wrap.appendChild(tool);
      wrap.appendChild(chartWrap);
      body.textContent = "";
      body.appendChild(wrap);
      const rangeSel = tool.querySelector("#drill-range-mem") as HTMLSelectElement;
      const loadChart = async () => {
        const ser = await fetchHostStatsSeries("memory_used", rangeSel.value);
        uroot.textContent = "";
        drawDrillSingle(uroot, ser, "memory");
      };
      rangeSel.addEventListener("change", () => void loadChart());
      await loadChart();
      scheduleDrillChartRefresh(() => void loadChart());

      const d: MemoryDetail = await fetchHostStatsDetail("memory", { top: 20 });
      const m = d.memory;
      const sum = document.createElement("p");
      sum.style.fontSize = "0.82rem";
      sum.textContent = `${formatBytes(m.used_bytes)} / ${formatBytes(m.total_bytes)} · swap ${formatBytes(m.swap_used_bytes)} / ${formatBytes(m.swap_total_bytes)}`;
      body.appendChild(sum);
      const tbl = document.createElement("table");
      tbl.className = "host-stats-detail";
      const thead = document.createElement("thead");
      thead.innerHTML = `<tr><th data-sort-key="0">PID</th><th data-sort-key="1">Name</th><th data-sort-key="2">RSS</th></tr>`;
      tbl.appendChild(thead);
      const tb = document.createElement("tbody");
      for (const p of d.top_processes) {
        const tr = document.createElement("tr");
        tr.innerHTML = `<td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${escapeHtml(formatBytes(p.memory_bytes))}</td>`;
        tb.appendChild(tr);
      }
      tbl.appendChild(tb);
      body.appendChild(tbl);
      bindSortableTable(tbl);
    } else if (kind === "disk") {
      const d: DiskDetail = await fetchHostStatsDetail("disk", { top: 20 });
      body.textContent = "";
      const note = document.createElement("p");
      note.className = "muted";
      note.style.fontSize = "0.8rem";
      note.textContent = t("status.hostServer.diskIoSoon");
      body.appendChild(note);
      if (d.io?.note) {
        const p2 = document.createElement("p");
        p2.className = "muted";
        p2.style.fontSize = "0.75rem";
        p2.textContent = d.io.note;
        body.appendChild(p2);
      }
      const mounts = d.mounts;
      const { primary, other } = filterMountsForDisplay(mounts, overview.disk_mount_path);
      const mh = document.createElement("h3");
      mh.style.fontSize = "0.85rem";
      mh.textContent = t("status.hostServer.mountsHeading");
      body.appendChild(mh);
      const t1 = document.createElement("table");
      t1.className = "host-stats-detail";
      t1.innerHTML = `<thead><tr>
        <th data-sort-key="0">Mount</th><th data-sort-key="1">Free</th><th data-sort-key="2">Total</th>
      </tr></thead><tbody></tbody>`;
      const tb1 = t1.querySelector("tbody")!;
      for (const m of primary) {
        const tr = document.createElement("tr");
        tr.innerHTML = `<td>${escapeHtml(m.path)}</td><td>${escapeHtml(formatBytes(m.free_bytes))}</td><td>${escapeHtml(formatBytes(m.total_bytes))}</td>`;
        tb1.appendChild(tr);
      }
      body.appendChild(t1);
      bindSortableTable(t1);
      if (other.length > 0) {
        const det = document.createElement("details");
        const sm = document.createElement("summary");
        sm.textContent = t("status.hostServer.otherMounts", { count: String(other.length) });
        det.appendChild(sm);
        const t2 = document.createElement("table");
        t2.className = "host-stats-detail";
        t2.style.marginTop = "0.35rem";
        t2.innerHTML = `<thead><tr><th>Mount</th><th>Free</th><th>Total</th></tr></thead><tbody></tbody>`;
        const tb2 = t2.querySelector("tbody")!;
        for (const m of other) {
          const tr = document.createElement("tr");
          tr.innerHTML = `<td>${escapeHtml(m.path)}</td><td>${escapeHtml(formatBytes(m.free_bytes))}</td><td>${escapeHtml(formatBytes(m.total_bytes))}</td>`;
          tb2.appendChild(tr);
        }
        det.appendChild(t2);
        body.appendChild(det);
      }
      const ph = document.createElement("h3");
      ph.style.fontSize = "0.85rem";
      ph.textContent = "Top I/O (process)";
      body.appendChild(ph);
      const pt = document.createElement("table");
      pt.className = "host-stats-detail";
      pt.innerHTML = `<thead><tr>
        <th data-sort-key="0">PID</th><th data-sort-key="1">Name</th><th data-sort-key="2">Read</th><th data-sort-key="3">Write</th>
      </tr></thead><tbody></tbody>`;
      const ptb = pt.querySelector("tbody")!;
      for (const p of d.top_processes) {
        const tr = document.createElement("tr");
        tr.innerHTML = `<td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${escapeHtml(formatBytes(p.read_bytes))}</td><td>${escapeHtml(formatBytes(p.write_bytes))}</td>`;
        ptb.appendChild(tr);
      }
      body.appendChild(pt);
      bindSortableTable(pt);
    } else if (kind === "network") {
      const wrap = document.createElement("div");
      const tool = document.createElement("div");
      tool.className = "host-drill-toolbar-row";
      tool.innerHTML = `<label>${escapeHtml(t("status.hostServer.chartRange"))}
        <select id="drill-range-net" class="host-drill-limit">
          <option value="15m">15m</option>
          <option value="1h" selected>1h</option>
          <option value="24h">24h</option>
        </select></label>`;
      const chartWrap = document.createElement("div");
      chartWrap.className = "host-drill-chart-wrap";
      const uroot = document.createElement("div");
      uroot.className = "host-drill-uplot";
      chartWrap.appendChild(uroot);
      wrap.appendChild(tool);
      wrap.appendChild(chartWrap);
      body.textContent = "";
      body.appendChild(wrap);
      const rangeSel = tool.querySelector("#drill-range-net") as HTMLSelectElement;
      const loadChart = async () => {
        const r = rangeSel.value;
        const [a, b] = await Promise.all([
          fetchHostStatsSeries("net_rx", r),
          fetchHostStatsSeries("net_tx", r),
        ]);
        uroot.textContent = "";
        drawDrillDualNet(uroot, a, b);
      };
      rangeSel.addEventListener("change", () => void loadChart());
      await loadChart();
      scheduleDrillChartRefresh(() => void loadChart());

      const d: NetworkDetail = await fetchHostStatsDetail("network", undefined);
      const note = document.createElement("p");
      note.className = "muted";
      note.style.fontSize = "0.78rem";
      note.textContent = d.connections_note;
      body.appendChild(note);
      const tbl = document.createElement("table");
      tbl.className = "host-stats-detail";
      const thead = document.createElement("thead");
      thead.innerHTML = `<tr><th data-sort-key="0">IF</th><th data-sort-key="1">RX/s</th><th data-sort-key="2">TX/s</th><th data-sort-key="3">err</th></tr>`;
      tbl.appendChild(thead);
      const tb = document.createElement("tbody");
      for (const i of d.interfaces) {
        const tr = document.createElement("tr");
        tr.innerHTML = `<td>${escapeHtml(i.name)}</td><td>${escapeHtml(formatRateBps(i.rx_bytes_per_s))}</td><td>${escapeHtml(formatRateBps(i.tx_bytes_per_s))}</td><td>${i.rx_errors} / ${i.tx_errors}</td>`;
        tb.appendChild(tr);
      }
      tbl.appendChild(tb);
      body.appendChild(tbl);
      bindSortableTable(tbl);
    } else if (kind === "processes") {
      body.textContent = "";
      const toolbar = document.createElement("div");
      toolbar.className = "host-drill-toolbar-row";
      const search = document.createElement("input");
      search.type = "search";
      search.className = "host-drill-search";
      search.placeholder = t("status.hostServer.procSearch");
      const lim = document.createElement("select");
      lim.className = "host-drill-limit";
      for (const n of [50, 100, 200]) {
        const o = document.createElement("option");
        o.value = String(n);
        o.textContent = `Top ${n}`;
        if (n === 100) {
          o.selected = true;
        }
        lim.appendChild(o);
      }
      toolbar.appendChild(search);
      toolbar.appendChild(lim);
      body.appendChild(toolbar);
      const tableHost = document.createElement("div");
      tableHost.id = "host-drill-proc-table";
      body.appendChild(tableHost);

      let debounce: ReturnType<typeof setTimeout> | null = null;
      const loadProcs = async () => {
        const d: ProcessesDetail = await fetchHostStatsDetail("processes", {
          limit: parseInt(lim.value, 10),
          q: search.value.trim(),
        });
        tableHost.textContent = "";
        const total = document.createElement("p");
        total.style.fontSize = "0.78rem";
        total.className = "muted";
        total.textContent = `total ${d.total}`;
        tableHost.appendChild(total);
        const tbl = document.createElement("table");
        tbl.className = "host-stats-detail";
        const thead = document.createElement("thead");
        thead.innerHTML = `<tr><th data-sort-key="0">PID</th><th data-sort-key="1">Name</th><th data-sort-key="2">CPU %</th><th data-sort-key="3">Mem</th></tr>`;
        tbl.appendChild(thead);
        const tb = document.createElement("tbody");
        for (const p of d.processes) {
          const tr = document.createElement("tr");
          if (p.cpu_percent >= 20 || p.memory_bytes > 512 * 1024 * 1024) {
            tr.classList.add("host-stats-row--high");
          }
          tr.innerHTML = `<td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${p.cpu_percent.toFixed(1)}%</td><td>${escapeHtml(formatBytes(p.memory_bytes))}</td>`;
          tb.appendChild(tr);
        }
        tbl.appendChild(tb);
        tableHost.appendChild(tbl);
        bindSortableTable(tbl);
      };
      search.addEventListener("input", () => {
        if (debounce) {
          clearTimeout(debounce);
        }
        debounce = setTimeout(() => void loadProcs(), 200);
      });
      lim.addEventListener("change", () => void loadProcs());
      await loadProcs();
    }
  } catch (e) {
    body.innerHTML = `<p class="host-stats-error">${escapeHtml(formatErr(e))}</p>`;
  }
}

async function tryInitCharts(): Promise<void> {
  const wrap = document.getElementById("host-server-stats-charts");
  const metricSel = document.getElementById(
    "host-server-chart-metric",
  ) as HTMLSelectElement | null;
  const rangeSel = document.getElementById(
    "host-server-chart-range",
  ) as HTMLSelectElement | null;
  const uroot = document.getElementById("host-server-uplot");
  if (!wrap || !metricSel || !rangeSel || !uroot) {
    return;
  }
  if (chartsAvailable === false) {
    wrap.hidden = true;
    return;
  }
  if (chartsAvailable === null) {
    try {
      await fetchHostStatsSeries("cpu", "1h");
      chartsAvailable = true;
    } catch {
      chartsAvailable = false;
      wrap.hidden = true;
      return;
    }
  }
  wrap.hidden = false;
  if (metricSel.options.length === 0) {
    const metrics: [string, string][] = [
      ["cpu", "status.hostServer.metricLabel.cpu"],
      ["memory_used", "status.hostServer.metricLabel.memory_used"],
      ["load1", "status.hostServer.metricLabel.load1"],
      ["net_rx", "status.hostServer.metricLabel.net_rx"],
      ["net_tx", "status.hostServer.metricLabel.net_tx"],
    ];
    for (const [v, key] of metrics) {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = t(key as MessageKey);
      metricSel.appendChild(o);
    }
  }

  if (!chartToolbarBound) {
    chartToolbarBound = true;
    metricSel.addEventListener("change", () => {
      void refreshHostChartIfReady();
    });
    rangeSel.addEventListener("change", () => {
      void refreshHostChartIfReady();
    });
  }
  await refreshHostChartIfReady();
}

function stopPolling(): void {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

function stopSse(): void {
  if (sseAbort) {
    sseAbort.abort();
    sseAbort = null;
  }
}

function startPolling(onTick: () => void): void {
  stopPolling();
  pollTimer = setInterval(onTick, 5000);
}

function applyOverview(data: HostStatsView): void {
  lastOverview = data;
  pushSparkFromOverview(data);
  const cards = document.getElementById("host-server-stats-cards");
  if (cards) {
    const canPatch = Boolean(cards.querySelector("button.host-stats-card"));
    if (canPatch && patchCardsInPlace(cards, data)) {
      /* ok */
    } else {
      renderCardsFull(cards, data);
    }
  }
  renderSecondaryTabs(data);
  const badge = document.getElementById("host-server-live-badge");
  const liveSse = document.getElementById(
    "host-server-live-sse",
  ) as HTMLInputElement | null;
  if (badge) {
    badge.hidden = !liveSse?.checked;
  }
}

function scheduleApplyOverview(data: HostStatsView): void {
  ssePending = data;
  if (sseRaf) {
    return;
  }
  sseRaf = requestAnimationFrame(() => {
    sseRaf = 0;
    if (ssePending) {
      applyOverview(ssePending);
      ssePending = null;
    }
  });
}

function setFetchStatus(message: string): void {
  const el = document.getElementById("host-server-fetch-status");
  if (el) {
    el.textContent = message;
  }
}

type LoadMode = "initial" | "refresh" | "manual";

async function loadIntoDialog(mode: LoadMode = "initial"): Promise<void> {
  const cards = document.getElementById("host-server-stats-cards");
  const dialog = document.getElementById("dialog-host-server") as HTMLDialogElement | null;
  if (!cards || !dialog?.open) {
    return;
  }

  if (mode === "initial") {
    cards.innerHTML = `<div class="host-stats-loading">${escapeHtml(t("status.hostServer.loading"))}</div>`;
    const st = document.getElementById("host-server-tab-storage");
    const nt = document.getElementById("host-server-tab-network");
    const lt = document.getElementById("host-server-tab-logs");
    if (st) {
      st.textContent = "";
    }
    if (nt) {
      nt.textContent = "";
    }
    if (lt) {
      lt.textContent = "";
    }
    setFetchStatus("");
    cpuSpark.length = 0;
    netSpark.length = 0;
  }

  try {
    const data = await client.fetch();
    setFetchStatus("");
    applyOverview(data);
    if (mode === "initial") {
      chartsTabInited = false;
      void tryInitChartsWhenChartsTab();
    } else {
      void refreshHostChartIfReady();
    }
  } catch (e) {
    if (mode === "initial") {
      cards.innerHTML = `<div class="host-stats-error">${escapeHtml(formatErr(e))}</div>`;
    } else {
      setFetchStatus(formatErr(e));
    }
  }
}

async function tryInitChartsWhenChartsTab(): Promise<void> {
  const panel = document.getElementById("panel-host-charts");
  if (!panel || panel.hidden) {
    return;
  }
  if (!chartsTabInited) {
    chartsTabInited = true;
    await tryInitCharts();
  }
}

async function refreshHostChartIfReady(): Promise<void> {
  if (chartsAvailable !== true) {
    return;
  }
  const panel = document.getElementById("panel-host-charts");
  if (panel?.hidden) {
    return;
  }
  const metricSel = document.getElementById(
    "host-server-chart-metric",
  ) as HTMLSelectElement | null;
  const rangeSel = document.getElementById(
    "host-server-chart-range",
  ) as HTMLSelectElement | null;
  const uroot = document.getElementById("host-server-uplot");
  if (!metricSel || !rangeSel || !uroot) {
    return;
  }
  const metric = metricSel.value;
  const range = rangeSel.value;
  try {
    const ser = await fetchHostStatsSeries(metric, range);
    uroot.textContent = "";
    drawMainChart(uroot, ser);
  } catch {
    uroot.textContent = t("status.hostServer.chartsUnavailable");
  }
}

function parseSseDataBlock(block: string): string | null {
  const normalized = block.replace(/\r\n/g, "\n");
  const lines = normalized.split("\n");
  const parts: string[] = [];
  for (const line of lines) {
    if (line.startsWith("data:")) {
      parts.push(line.slice(5).trimStart());
    }
  }
  if (parts.length === 0) {
    return null;
  }
  return parts.join("\n");
}

function setSseStatus(text: string): void {
  const el = document.getElementById("host-server-sse-status");
  if (el) {
    el.textContent = text;
  }
}

async function consumeHostStatsSseStream(
  body: ReadableStream<Uint8Array>,
  onJson: (data: HostStatsView) => void,
): Promise<void> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  while (true) {
    const { done, value } = await reader.read();
    if (value) {
      buf += decoder.decode(value, { stream: true });
    }
    buf = buf.replace(/\r\n/g, "\n");
    let sep: number;
    while ((sep = buf.indexOf("\n\n")) >= 0) {
      const block = buf.slice(0, sep);
      buf = buf.slice(sep + 2);
      if (block.startsWith(":") || block.trim() === "") {
        continue;
      }
      const payload = parseSseDataBlock(block);
      if (!payload) {
        continue;
      }
      try {
        onJson(JSON.parse(payload) as HostStatsView);
      } catch {
        /* ignore */
      }
    }
    if (done) {
      break;
    }
  }
}

function bindLiveSse(checkbox: HTMLInputElement): void {
  checkbox.addEventListener("change", () => {
    stopSse();
    setSseStatus("");
    const badge = document.getElementById("host-server-live-badge");
    if (badge) {
      badge.hidden = !checkbox.checked;
    }
    if (!checkbox.checked) {
      return;
    }
    void (async () => {
      const token = apiToken();
      const headers: Record<string, string> = {
        Accept: "text/event-stream",
      };
      if (token) {
        headers.Authorization = `Bearer ${token}`;
      }
      const ctrl = new AbortController();
      sseAbort = ctrl;
      try {
        const res = await fetch(
          `${window.location.origin}/api/v1/host-stats/stream`,
          {
            method: "GET",
            headers,
            signal: ctrl.signal,
            cache: "no-store",
          },
        );
        if (!res.ok) {
          checkbox.checked = false;
          if (badge) {
            badge.hidden = true;
          }
          const msg =
            res.status === 503
              ? t("status.hostServer.sseErr503")
              : res.status === 401
                ? t("status.hostServer.sseErr401")
                : t("status.hostServer.sseErrHttp", { status: String(res.status) });
          setSseStatus(msg);
          return;
        }
        const body = res.body;
        if (!body) {
          checkbox.checked = false;
          if (badge) {
            badge.hidden = true;
          }
          setSseStatus(t("status.hostServer.sseErrNoBody"));
          return;
        }
        setSseStatus(t("status.hostServer.sseConnected"));
        await consumeHostStatsSseStream(body, (data) => {
          scheduleApplyOverview(data);
        });
        if (!ctrl.signal.aborted) {
          setSseStatus(t("status.hostServer.sseEnded"));
          checkbox.checked = false;
          if (badge) {
            badge.hidden = true;
          }
        }
      } catch (e) {
        if (
          (e instanceof DOMException || e instanceof Error) &&
          e.name === "AbortError"
        ) {
          setSseStatus("");
          return;
        }
        checkbox.checked = false;
        if (badge) {
          badge.hidden = true;
        }
        setSseStatus(
          e instanceof Error ? e.message : t("status.hostServer.sseErrUnknown"),
        );
      } finally {
        sseAbort = null;
      }
    })();
  });
}

function bindHostTabs(): void {
  const tablist = document.querySelector(".host-stats-tablist");
  if (!tablist) {
    return;
  }
  const tabs = tablist.querySelectorAll<HTMLButtonElement>(".host-stats-tab");
  const panels: Record<string, HTMLElement | null> = {
    storage: document.getElementById("panel-host-storage"),
    network: document.getElementById("panel-host-network"),
    logs: document.getElementById("panel-host-logs"),
    charts: document.getElementById("panel-host-charts"),
  };
  const activate = (name: string): void => {
    for (const t of tabs) {
      const on = t.dataset.tab === name;
      t.setAttribute("aria-selected", on ? "true" : "false");
    }
    for (const [k, p] of Object.entries(panels)) {
      if (p) {
        p.hidden = k !== name;
      }
    }
    if (name === "charts") {
      void tryInitChartsWhenChartsTab().then(() => refreshHostChartIfReady());
    }
  };
  tablist.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest(
      ".host-stats-tab",
    ) as HTMLButtonElement | null;
    if (!btn?.dataset.tab) {
      return;
    }
    activate(btn.dataset.tab);
  });
  activate("storage");
}

function bindMainUplotResize(): void {
  const uroot = document.getElementById("host-server-uplot");
  const wrap = document.getElementById("host-server-stats-charts");
  if (!uroot || !wrap) {
    return;
  }
  const ro = new ResizeObserver(() => {
    if (!mainUplot || wrap.hidden) {
      return;
    }
    const w = uroot.clientWidth;
    if (w > 40) {
      mainUplot.setSize({ width: w, height: mainUplot.height });
    }
  });
  ro.observe(uroot);
}

export function bindHostServerDialog(): void {
  const btnOpen = document.getElementById("btn-host-server-info");
  const dialog = document.getElementById("dialog-host-server") as HTMLDialogElement | null;
  const btnClose = document.getElementById("btn-host-server-close");
  const btnRefresh = document.getElementById("btn-host-server-refresh");
  const btnDrillClose = document.getElementById("btn-host-server-drill-close");
  const drillDlg = document.getElementById(
    "dialog-host-server-drill",
  ) as HTMLDialogElement | null;
  const liveSse = document.getElementById(
    "host-server-live-sse",
  ) as HTMLInputElement | null;
  if (!btnOpen || !dialog || !btnClose || !btnRefresh) {
    return;
  }

  bindHostTabs();
  bindMainUplotResize();

  if (liveSse) {
    bindLiveSse(liveSse);
  }

  btnDrillClose?.addEventListener("click", () => {
    destroyDrillUplot();
    drillDlg?.close();
  });
  drillDlg?.addEventListener("close", () => {
    destroyDrillUplot();
    const b = document.getElementById("host-server-drill-body");
    if (b) {
      b.textContent = "";
    }
  });

  const cardsGrid = document.getElementById("host-server-stats-cards");
  cardsGrid?.addEventListener("click", (ev) => {
    const tEl = ev.target as HTMLElement;
    const btn = tEl.closest("button.host-stats-card") as HTMLButtonElement | null;
    if (!btn?.dataset.kind || !lastOverview) {
      return;
    }
    activeCard = btn.dataset.kind as CardKind;
    for (const el of cardsGrid.querySelectorAll("button.host-stats-card")) {
      const b = el as HTMLButtonElement;
      b.classList.toggle("is-active", b.dataset.kind === activeCard);
    }
    void openDrillModal(activeCard, lastOverview);
  });

  btnOpen.addEventListener("click", () => {
    activeCard = null;
    lastOverview = null;
    chartsAvailable = null;
    chartsTabInited = false;
    showAllNetIf = false;
    dialog.showModal();
    void loadIntoDialog("initial");
    stopPolling();
    startPolling(() => {
      if (dialog.open && !liveSse?.checked) {
        void loadIntoDialog("refresh");
      }
    });
  });

  btnClose.addEventListener("click", () => {
    dialog.close();
  });

  btnRefresh.addEventListener("click", () => {
    void loadIntoDialog("manual");
  });

  dialog.addEventListener("close", () => {
    stopPolling();
    stopSse();
    setSseStatus("");
    setFetchStatus("");
    lastOverview = null;
    destroyMainUplot();
    destroyDrillUplot();
    drillDlg?.close();
    if (liveSse) {
      liveSse.checked = false;
    }
    const badge = document.getElementById("host-server-live-badge");
    if (badge) {
      badge.hidden = true;
    }
    activeCard = null;
    ssePending = null;
    sseRaf = 0;
  });
}
