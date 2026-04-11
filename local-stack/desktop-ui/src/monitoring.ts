import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

export type MonitoringOverview = {
  ts_ms: number;
  disk: { mounts: { path: string; total_bytes: number; free_bytes: number }[] };
  memory: {
    total_bytes: number;
    used_bytes: number;
    available_bytes: number;
    cached_bytes?: number | null;
    buffers_bytes?: number | null;
    swap_total_bytes: number;
    swap_used_bytes: number;
  };
  cpu: { usage_percent: number; loadavg: { m1: number; m5: number; m15: number } };
  temperature_c?: { current_max: number; avg: number } | null;
  process_count: number;
  network: {
    interfaces: {
      name: string;
      rx_bytes_per_s: number;
      tx_bytes_per_s: number;
      rx_errors: number;
      tx_errors: number;
    }[];
  };
  logs: { items: { ts_ms: number; level: string; message: string }[] };
  warnings: string[];
  partial?: boolean;
};

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${u[i]}`;
}

function httpToWs(base: string): string {
  return base.replace(/^http/, "ws");
}

let wsRef: WebSocket | null = null;
let pollTimer: ReturnType<typeof setInterval> | null = null;

export function stopMonitoringStreams(): void {
  if (wsRef) {
    wsRef.close();
    wsRef = null;
  }
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

export async function fetchOverview(base: string): Promise<MonitoringOverview> {
  const r = await fetch(`${base}/api/v1/monitoring/overview`);
  if (!r.ok) {
    throw new Error(`overview ${r.status}`);
  }
  return r.json() as Promise<MonitoringOverview>;
}

export function startMonitoringStreams(
  base: string,
  onOverview: (o: MonitoringOverview) => void,
  onErr: (e: string) => void,
): void {
  stopMonitoringStreams();
  const url = `${httpToWs(base)}/api/v1/monitoring/stream`;
  try {
    const ws = new WebSocket(url);
    wsRef = ws;
    ws.onmessage = (ev) => {
      try {
        const j = JSON.parse(String(ev.data)) as {
          type?: string;
          payload?: MonitoringOverview;
        };
        if (j.type === "tick" && j.payload) {
          onOverview(j.payload);
        }
      } catch {
        /* ignore */
      }
    };
    ws.onerror = () => {
      onErr("WebSocket error; using polling");
    };
    ws.onclose = () => {
      wsRef = null;
    };
  } catch {
    onErr("WebSocket unavailable");
  }
  pollTimer = setInterval(() => {
    void fetchOverview(base).then(onOverview).catch((e: unknown) => onErr(String(e)));
  }, 3000);
  void fetchOverview(base).then(onOverview).catch((e: unknown) => onErr(String(e)));
}

export function monitoringDashboardHtml(
  base: string | null,
  data: MonitoringOverview | null,
  err: string | null,
): string {
  if (!base) {
    return `
      <div class="card">
        <div class="card-inner">
          <h2>Server information</h2>
          <p class="meta warn">Monitoring API is not available.</p>
        </div>
      </div>`;
  }
  if (err && !data) {
    return `
      <div class="card">
        <div class="card-inner">
          <h2>Server information</h2>
          <p class="err-text">${escapeHtml(err)}</p>
        </div>
      </div>`;
  }
  if (!data) {
    return `
      <div class="card">
        <div class="card-inner">
          <h2>Server information</h2>
          <p class="meta">Loading metrics…</p>
        </div>
      </div>`;
  }

  const mainMount = data.disk.mounts[0];
  const diskLine = mainMount
    ? `${formatBytes(mainMount.free_bytes)} free of ${formatBytes(mainMount.total_bytes)} (${escapeHtml(mainMount.path)})`
    : "—";

  const tempLine = data.temperature_c
    ? `${data.temperature_c.current_max.toFixed(1)} °C max · ${data.temperature_c.avg.toFixed(1)} °C avg`
    : "n/a";

  const netSum = data.network.interfaces.reduce(
    (a, i) => ({
      rx: a.rx + i.rx_bytes_per_s,
      tx: a.tx + i.tx_bytes_per_s,
    }),
    { rx: 0, tx: 0 },
  );

  const warn =
    data.warnings.length > 0
      ? `<p class="meta warn">${data.warnings.map((w) => escapeHtml(w)).join("<br/>")}</p>`
      : "";

  const logsPreview = data.logs.items
    .slice(-5)
    .map((l) => `<li class="log-li"><span class="log-lvl">${escapeHtml(l.level)}</span> ${escapeHtml(l.message)}</li>`)
    .join("");

  return `
    <div class="card monitoring-card">
      <div class="card-inner">
        <h2>Server information</h2>
        <p class="meta small">Local host metrics · <code>${escapeHtml(base)}</code></p>
        ${warn}
        <div class="mon-grid" data-mon-base="${escapeHtml(base)}">
          <button type="button" class="mon-tile" data-mon-detail="cpu">
            <span class="mon-tile-label">CPU</span>
            <span class="mon-tile-val">${data.cpu.usage_percent.toFixed(1)}%</span>
            <span class="mon-tile-sub">load ${data.cpu.loadavg.m1.toFixed(2)} / ${data.cpu.loadavg.m5.toFixed(2)} / ${data.cpu.loadavg.m15.toFixed(2)}</span>
          </button>
          <button type="button" class="mon-tile" data-mon-detail="memory">
            <span class="mon-tile-label">Memory</span>
            <span class="mon-tile-val">${formatBytes(data.memory.used_bytes)}</span>
            <span class="mon-tile-sub">of ${formatBytes(data.memory.total_bytes)}</span>
          </button>
          <button type="button" class="mon-tile" data-mon-detail="disk">
            <span class="mon-tile-label">Disk</span>
            <span class="mon-tile-val">${diskLine}</span>
          </button>
          <button type="button" class="mon-tile" data-mon-detail="network">
            <span class="mon-tile-label">Network</span>
            <span class="mon-tile-val">↓ ${formatBytes(netSum.rx)}/s</span>
            <span class="mon-tile-sub">↑ ${formatBytes(netSum.tx)}/s (sum)</span>
          </button>
          <button type="button" class="mon-tile" data-mon-detail="temp">
            <span class="mon-tile-label">Temperature</span>
            <span class="mon-tile-val">${escapeHtml(tempLine)}</span>
          </button>
          <button type="button" class="mon-tile" data-mon-detail="processes">
            <span class="mon-tile-label">Processes</span>
            <span class="mon-tile-val">${data.process_count}</span>
          </button>
          <button type="button" class="mon-tile" data-mon-detail="logs">
            <span class="mon-tile-label">Logs</span>
            <span class="mon-tile-val">${data.logs.items.length} lines</span>
            <span class="mon-tile-sub">app log tail</span>
          </button>
        </div>
        <ul class="log-preview">${logsPreview}</ul>
        <div class="btn-row">
          <button type="button" class="btn btn-ghost btn-sm" data-action="mon-export">Export JSON</button>
          <button type="button" class="btn btn-ghost btn-sm" data-action="mon-alerts">Alerts</button>
        </div>
        <div id="mon-detail-host"></div>
      </div>
    </div>`;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

let uplotInst: uPlot | null = null;

/** Tear down uPlot when closing detail panel. */
export function destroyMonitoringChart(): void {
  if (uplotInst) {
    uplotInst.destroy();
    uplotInst = null;
  }
}

export async function openDetail(
  base: string,
  kind: string,
  host: HTMLElement,
): Promise<void> {
  destroyMonitoringChart();
  host.innerHTML = `<p class="meta">Loading…</p>`;
  try {
    if (kind === "cpu") {
      const r = await fetch(`${base}/api/v1/monitoring/detail/cpu`);
      const j = (await r.json()) as {
        top_processes: { pid: number; name: string; cpu_percent: number }[];
        loadavg: { m1: number; m5: number; m15: number };
        times?: { user_ms: number; system_ms: number; idle_ms: number };
      };
      const rows = j.top_processes
        .map((p) => `<tr><td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${p.cpu_percent.toFixed(1)}%</td></tr>`)
        .join("");
      const times = j.times
        ? `<p class="meta">user ${j.times.user_ms} ms · system ${j.times.system_ms} ms · idle ${j.times.idle_ms} ms (Linux jiffies-based)</p>`
        : "";
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">CPU detail</h3>
          ${times}
          <p class="meta">load ${j.loadavg.m1.toFixed(2)} / ${j.loadavg.m5.toFixed(2)} / ${j.loadavg.m15.toFixed(2)}</p>
          <div id="mon-chart" class="mon-chart"></div>
          <table class="mon-table"><thead><tr><th>PID</th><th>Name</th><th>CPU %</th></tr></thead><tbody>${rows}</tbody></table>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
      await drawSeries(base, host.querySelector("#mon-chart") as HTMLElement, "cpu");
    } else if (kind === "memory") {
      const r = await fetch(`${base}/api/v1/monitoring/detail/memory`);
      const j = (await r.json()) as {
        memory: MonitoringOverview["memory"];
        top_processes: { pid: number; name: string; memory_bytes: number }[];
      };
      const rows = j.top_processes
        .map(
          (p) =>
            `<tr><td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${formatBytes(p.memory_bytes)}</td></tr>`,
        )
        .join("");
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">Memory detail</h3>
          <p class="meta">used ${formatBytes(j.memory.used_bytes)} · available ${formatBytes(j.memory.available_bytes)} · swap ${formatBytes(j.memory.swap_used_bytes)} / ${formatBytes(j.memory.swap_total_bytes)}</p>
          <div id="mon-chart" class="mon-chart"></div>
          <table class="mon-table"><thead><tr><th>PID</th><th>Name</th><th>RSS</th></tr></thead><tbody>${rows}</tbody></table>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
      await drawSeries(base, host.querySelector("#mon-chart") as HTMLElement, "memory_used");
    } else if (kind === "disk") {
      const r = await fetch(`${base}/api/v1/monitoring/detail/disk`);
      const j = (await r.json()) as {
        mounts: { path: string; total_bytes: number; free_bytes: number }[];
        top_processes: { pid: number; name: string; read_bytes: number; write_bytes: number }[];
        io?: { note: string };
      };
      const mounts = j.mounts
        .map(
          (m) =>
            `<tr><td>${escapeHtml(m.path)}</td><td>${formatBytes(m.free_bytes)}</td><td>${formatBytes(m.total_bytes)}</td></tr>`,
        )
        .join("");
      const rows = j.top_processes
        .map(
          (p) =>
            `<tr><td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${formatBytes(p.read_bytes)}</td><td>${formatBytes(p.write_bytes)}</td></tr>`,
        )
        .join("");
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">Disk detail</h3>
          ${j.io ? `<p class="meta small">${escapeHtml(j.io.note)}</p>` : ""}
          <table class="mon-table"><thead><tr><th>Mount</th><th>Free</th><th>Total</th></tr></thead><tbody>${mounts}</tbody></table>
          <h4 class="subhead">Top I/O (process)</h4>
          <table class="mon-table"><thead><tr><th>PID</th><th>Name</th><th>Read</th><th>Write</th></tr></thead><tbody>${rows}</tbody></table>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
    } else if (kind === "network") {
      const r = await fetch(`${base}/api/v1/monitoring/detail/network`);
      const j = (await r.json()) as {
        interfaces: MonitoringOverview["network"]["interfaces"];
        connections_note: string;
      };
      const rows = j.interfaces
        .map(
          (i) =>
            `<tr><td>${escapeHtml(i.name)}</td><td>${formatBytes(i.rx_bytes_per_s)}/s</td><td>${formatBytes(i.tx_bytes_per_s)}/s</td><td>${i.rx_errors}</td><td>${i.tx_errors}</td></tr>`,
        )
        .join("");
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">Network detail</h3>
          <p class="meta small">${escapeHtml(j.connections_note)}</p>
          <div id="mon-chart" class="mon-chart"></div>
          <table class="mon-table"><thead><tr><th>Iface</th><th>RX/s</th><th>TX/s</th><th>err in</th><th>err out</th></tr></thead><tbody>${rows}</tbody></table>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
      await drawSeries(base, host.querySelector("#mon-chart") as HTMLElement, "net_rx");
    } else if (kind === "temp") {
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">Temperature</h3>
          <p class="meta">See overview card; sensors vary by platform.</p>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
    } else if (kind === "processes") {
      const r = await fetch(`${base}/api/v1/monitoring/detail/processes?limit=80`);
      const j = (await r.json()) as {
        processes: { pid: number; name: string; cpu_percent: number; memory_bytes: number }[];
        total: number;
      };
      const rows = j.processes
        .map(
          (p) =>
            `<tr><td>${p.pid}</td><td>${escapeHtml(p.name)}</td><td>${p.cpu_percent.toFixed(1)}%</td><td>${formatBytes(p.memory_bytes)}</td></tr>`,
        )
        .join("");
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">Processes (${j.total})</h3>
          <table class="mon-table"><thead><tr><th>PID</th><th>Name</th><th>CPU</th><th>Mem</th></tr></thead><tbody>${rows}</tbody></table>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
    } else if (kind === "logs") {
      const r = await fetch(`${base}/api/v1/monitoring/detail/logs?limit=80&level=all`);
      const j = (await r.json()) as { items: { level: string; message: string }[] };
      const rows = j.items
        .map((l) => `<tr><td>${escapeHtml(l.level)}</td><td class="mono">${escapeHtml(l.message)}</td></tr>`)
        .join("");
      host.innerHTML = `
        <div class="mon-detail">
          <h3 class="subhead">Application log</h3>
          <table class="mon-table"><thead><tr><th>Level</th><th>Message</th></tr></thead><tbody>${rows}</tbody></table>
          <button type="button" class="btn btn-ghost btn-sm mon-close" data-action="mon-close-detail">Close</button>
        </div>`;
    }
  } catch (e) {
    host.innerHTML = `<p class="err-text">${escapeHtml(String(e))}</p>`;
  }
}

async function drawSeries(
  base: string,
  el: HTMLElement | null,
  metric: string,
): Promise<void> {
  if (!el) return;
  destroyMonitoringChart();
  try {
    const r = await fetch(
      `${base}/api/v1/monitoring/series?metric=${encodeURIComponent(metric)}&range=1h&step=5000`,
    );
    const j = (await r.json()) as { points: { ts_ms: number; value: number }[] };
    const pts = j.points;
    if (pts.length === 0) {
      el.textContent = "No history yet (wait ~1 min)";
      return;
    }
    const xs = pts.map((p) => p.ts_ms / 1000);
    const ys = pts.map((p) => p.value);
    const data: uPlot.AlignedData = [xs, ys];
    uplotInst = new uPlot(
      {
        width: el.clientWidth || 400,
        height: 140,
        series: [{}, { label: metric, stroke: "#c4a84a" }],
      },
      data,
      el,
    );
  } catch {
    el.textContent = "Chart unavailable";
  }
}
