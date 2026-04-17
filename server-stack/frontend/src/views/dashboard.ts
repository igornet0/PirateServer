import {
  fetchGrpcSessions,
  fetchHistory,
  fetchProjects,
  fetchReleases,
  fetchStatus,
} from "../api/client.js";
import type {
  DisplayTopologyDisplayView,
  GrpcSessionEventView,
  GrpcSessionPeerView,
  StatusView,
} from "../api/types.js";
import { ApiRequestError } from "../api/types.js";
import { t } from "../i18n/index.js";

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

function formatSessionsTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) {
    return iso;
  }
  return d.toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  });
}

function truncateCell(s: string, max: number): string {
  const x = s.trim();
  if (x.length <= max) {
    return x;
  }
  return `${x.slice(0, Math.max(0, max - 1))}…`;
}

function connectionKindLabel(k: number): string {
  if (k === 1) {
    return "proxy";
  }
  if (k === 2) {
    return "resource";
  }
  return "—";
}

function fmtBytesCompact(n: number): string {
  if (!n) {
    return "0";
  }
  if (n < 4096) {
    return String(n);
  }
  if (n < 1024 * 1024) {
    return `${(n / 1024).toFixed(1)} KiB`;
  }
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

function badgeVariantForState(state: string): "ok" | "warn" | "crit" | "muted" {
  const s = state.toLowerCase();
  if (s.includes("run") || s.includes("active") || s.includes("ready")) {
    return "ok";
  }
  if (s.includes("stop") || s.includes("idle") || s.includes("pause")) {
    return "warn";
  }
  if (s.includes("error") || s.includes("fail")) {
    return "crit";
  }
  return "muted";
}

function highlightJsonForPre(json: string): string {
  return escapeHtml(json).replace(
    /&quot;([^&quot;]*)&quot;(\s*):/g,
    '<span class="json-key">&quot;$1&quot;</span>$2:',
  );
}

function metricBarHtml(pct: number | null | undefined, label: string): string {
  if (pct == null || Number.isNaN(pct)) {
    return `<span class="metric-empty">—</span>`;
  }
  const v = Math.min(100, Math.max(0, Math.round(pct)));
  let tone: "ok" | "warn" | "crit" = "ok";
  if (v >= 90) {
    tone = "crit";
  } else if (v >= 75) {
    tone = "warn";
  }
  return `<div class="metric-cell" role="img" aria-label="${escapeHtml(label)} ${String(v)}%">
    <div class="metric-bar-track"><div class="metric-bar-fill metric-bar-fill--${tone}" style="width:${String(v)}%"></div></div>
    <span class="metric-bar-val">${String(v)}%</span>
  </div>`;
}

function auditStatusBadge(status: string): string {
  let v: "ok" | "warn" | "crit" | "muted" = "muted";
  const low = status.toLowerCase();
  if (low.includes("ok") || low.includes("success") || low === "200" || low === "0") {
    v = "ok";
  } else if (low.includes("err") || low.includes("fail") || low.includes("denied") || low.includes("401")) {
    v = "crit";
  } else if (low.includes("warn")) {
    v = "warn";
  }
  return `<span class="badge badge--${v}">${escapeHtml(status)}</span>`;
}

/** Matches control-api default `online_secs` for display copy. */
const GRPC_ONLINE_SECS = 120;

let lastGrpcPeersForDisplayStream: GrpcSessionPeerView[] = [];
let displayStreamFormBound = false;
let displayStreamPreviewBound = false;

let lastPeersSnapshot: GrpcSessionPeerView[] = [];
let lastRawStatusCoreJson = "";
let peersToolbarBound = false;
let peerSearchDebounce: ReturnType<typeof setTimeout> | null = null;

/** `http://host:port/ingest` → `http://host:port` for `GET …/last.jpg`. */
export function ingestUrlToPreviewBase(ingest: string): string | null {
  const x = ingest.trim();
  if (!x.startsWith("http://") && !x.startsWith("https://")) {
    return null;
  }
  try {
    const u = new URL(x);
    const path = u.pathname.replace(/\/+$/, "") || "/";
    if (!path.endsWith("/ingest")) {
      return null;
    }
    const basePath = path.slice(0, -"/ingest".length) || "";
    return `${u.origin}${basePath}`;
  } catch {
    return null;
  }
}

function bindDisplayStreamPreview(): void {
  if (displayStreamPreviewBound) {
    return;
  }
  displayStreamPreviewBound = true;
  const dlg = document.getElementById("display-stream-preview-dialog") as HTMLDialogElement | null;
  const img = document.getElementById("display-stream-preview-img") as HTMLImageElement | null;
  const errEl = document.getElementById("display-stream-preview-err") as HTMLElement | null;
  const ingestEl = document.getElementById("display-stream-ingest-url") as HTMLInputElement | null;
  let previewTimer: ReturnType<typeof setInterval> | null = null;

  function stopPreview(): void {
    if (previewTimer != null) {
      clearInterval(previewTimer);
      previewTimer = null;
    }
    if (img) {
      img.removeAttribute("src");
    }
  }

  function tick(): void {
    if (!img || !dlg?.open) {
      return;
    }
    const base = ingestEl ? ingestUrlToPreviewBase(ingestEl.value) : null;
    if (!base) {
      return;
    }
    img.src = `${base}/last.jpg?t=${String(Date.now())}`;
  }

  document.getElementById("btn-display-stream-preview")?.addEventListener("click", () => {
    if (!dlg || !img || !ingestEl) {
      return;
    }
    const base = ingestUrlToPreviewBase(ingestEl.value);
    if (errEl) {
      errEl.hidden = true;
      errEl.textContent = "";
    }
    if (!base) {
      if (errEl) {
        errEl.textContent = t("status.displayStream.previewErr");
        errEl.hidden = false;
      }
      dlg.showModal();
      return;
    }
    stopPreview();
    img.crossOrigin = "anonymous";
    tick();
    previewTimer = setInterval(tick, 300);
    dlg.showModal();
  });

  dlg?.addEventListener("close", () => {
    stopPreview();
  });

  document.getElementById("btn-display-stream-preview-close")?.addEventListener("click", () => {
    dlg?.close();
  });
}

function displayStreamConfigObject(): Record<string, string | number> {
  const ingest =
    (document.getElementById("display-stream-ingest-url") as HTMLInputElement | null)?.value?.trim() ??
    "";
  const token =
    (document.getElementById("display-stream-token") as HTMLInputElement | null)?.value ?? "";
  const quality = Number(
    (document.getElementById("display-stream-quality") as HTMLInputElement | null)?.value ?? "70",
  );
  const fps = Number(
    (document.getElementById("display-stream-fps") as HTMLInputElement | null)?.value ?? "10",
  );
  const enc =
    (document.getElementById("display-stream-encrypt") as HTMLSelectElement | null)?.value ?? "none";
  const idxSel = document.getElementById("display-stream-display-index") as HTMLSelectElement | null;
  const display_index = Number(idxSel?.value ?? "0");
  const q = Math.min(100, Math.max(1, Number.isFinite(quality) ? quality : 70));
  const f = Math.min(60, Math.max(1, Number.isFinite(fps) ? fps : 10));
  const di = Number.isFinite(display_index) ? display_index : 0;
  return {
    v: 1,
    role: "producer",
    ingest_base_url: ingest,
    token: token.trim(),
    quality: q,
    fps: f,
    display_index: di,
    protocol: "http_post_jpeg",
    encrypt: enc === "tls" ? "tls" : "none",
  };
}

function refreshDisplayStreamDataUrl(): void {
  const pre = document.getElementById("display-stream-data-url");
  if (!pre) {
    return;
  }
  try {
    const json = JSON.stringify(displayStreamConfigObject());
    const b64 = btoa(unescape(encodeURIComponent(json)));
    pre.textContent = `data:application/json;base64,${b64}`;
  } catch (e) {
    pre.textContent = formatErr(e);
  }
}

function rebuildDisplayIndexOptions(
  idxSel: HTMLSelectElement,
  topology: DisplayTopologyDisplayView[] | undefined,
): void {
  idxSel.innerHTML = "";
  if (topology && topology.length > 0) {
    for (const d of topology) {
      const opt = document.createElement("option");
      opt.value = String(d.index);
      opt.textContent = `${d.index}: ${d.label} (${d.width}×${d.height})`;
      idxSel.appendChild(opt);
    }
    return;
  }
  for (let i = 0; i < 8; i++) {
    const opt = document.createElement("option");
    opt.value = String(i);
    opt.textContent = String(i);
    idxSel.appendChild(opt);
  }
}

function optionValuesMatch(sel: HTMLSelectElement, value: string): boolean {
  return Array.from(sel.options).some((o) => o.value === value);
}

function updateDisplayStreamPeerSelect(peers: GrpcSessionPeerView[]): void {
  lastGrpcPeersForDisplayStream = peers;
  const sel = document.getElementById("display-stream-peer") as HTMLSelectElement | null;
  const idxSel = document.getElementById("display-stream-display-index") as HTMLSelectElement | null;
  if (!sel || !idxSel) {
    return;
  }
  const prevPeerPk = sel.value;
  const prevDisplayIdx = idxSel.value;
  sel.replaceChildren();
  const opt0 = document.createElement("option");
  opt0.value = "";
  opt0.textContent = t("status.displayStream.peerManual");
  sel.appendChild(opt0);
  for (const p of peers) {
    const pk = p.client_public_key_b64?.trim() ?? "";
    if (!pk) {
      continue;
    }
    const opt = document.createElement("option");
    opt.value = pk;
    const short = pk.length > 16 ? `${pk.slice(0, 14)}…` : pk;
    opt.textContent = `${short} (${p.online ? t("status.sessions.online") : t("status.sessions.offline")})`;
    sel.appendChild(opt);
  }
  if (prevPeerPk && optionValuesMatch(sel, prevPeerPk)) {
    sel.value = prevPeerPk;
  }
  sel.onchange = () => {
    const pk = sel.value;
    const peer = lastGrpcPeersForDisplayStream.find((x) => x.client_public_key_b64 === pk);
    rebuildDisplayIndexOptions(idxSel, peer?.display_topology);
    refreshDisplayStreamDataUrl();
  };
  idxSel.onchange = () => refreshDisplayStreamDataUrl();
  const activePk = sel.value;
  const activePeer = lastGrpcPeersForDisplayStream.find((x) => x.client_public_key_b64 === activePk);
  rebuildDisplayIndexOptions(idxSel, activePeer?.display_topology);
  if (prevDisplayIdx && optionValuesMatch(idxSel, prevDisplayIdx)) {
    idxSel.value = prevDisplayIdx;
  }
  refreshDisplayStreamDataUrl();
}

function ensureDisplayStreamForm(): void {
  if (displayStreamFormBound) {
    return;
  }
  displayStreamFormBound = true;
  const ids = [
    "display-stream-ingest-url",
    "display-stream-token",
    "display-stream-quality",
    "display-stream-fps",
    "display-stream-encrypt",
  ];
  for (const id of ids) {
    document.getElementById(id)?.addEventListener("input", () => refreshDisplayStreamDataUrl());
    document.getElementById(id)?.addEventListener("change", () => refreshDisplayStreamDataUrl());
  }
  refreshDisplayStreamDataUrl();
  bindDisplayStreamPreview();
}

function renderGrpcAuditRows(rows: GrpcSessionEventView[]): string {
  return rows
    .map((r) => {
      const pk = r.client_public_key_b64?.trim() ?? "";
      const pkDisp = pk ? truncateCell(pk, 28) : "—";
      const det = truncateCell(r.detail ?? "", 48);
      return `<tr>
            <td class="cell-time">${escapeHtml(formatSessionsTime(r.created_at))}</td>
            <td><code>${escapeHtml(r.kind)}</code></td>
            <td><code class="cell-ip">${escapeHtml(r.peer_ip || "—")}</code></td>
            <td>${auditStatusBadge(r.status)}</td>
            <td><code>${escapeHtml(r.grpc_method || "—")}</code></td>
            <td><code class="cell-key">${escapeHtml(pkDisp)}</code></td>
            <td>${escapeHtml(det)}</td>
          </tr>`;
    })
    .join("");
}

function renderPeerRows(peers: GrpcSessionPeerView[]): string {
  return peers
    .map((p) => {
      const pkDisp = truncateCell(p.client_public_key_b64, 36);
      const presenceBadge = p.online
        ? `<span class="badge badge--ok">${escapeHtml(t("status.sessions.online"))}</span>`
        : `<span class="badge badge--offline">${escapeHtml(t("status.sessions.offline"))}</span>`;
      return `<tr>
            <td><code class="cell-key">${escapeHtml(pkDisp)}</code></td>
            <td><code>${escapeHtml(connectionKindLabel(p.connection_kind))}</code></td>
            <td class="cell-metric">${metricBarHtml(p.last_cpu_percent, "CPU")}</td>
            <td class="cell-metric">${metricBarHtml(p.last_ram_percent, "RAM")}</td>
            <td class="cell-metric">${metricBarHtml(p.last_gpu_percent, "GPU")}</td>
            <td class="cell-num">${escapeHtml(fmtBytesCompact(p.proxy_bytes_in_total))}</td>
            <td class="cell-num">${escapeHtml(fmtBytesCompact(p.proxy_bytes_out_total))}</td>
            <td><code class="cell-ip">${escapeHtml(p.last_peer_ip || "—")}</code></td>
            <td><code>${escapeHtml(p.last_grpc_method || "—")}</code></td>
            <td class="cell-time">${escapeHtml(formatSessionsTime(p.last_seen_at))}</td>
            <td>${presenceBadge}</td>
          </tr>`;
    })
    .join("");
}

function filterPeers(
  peers: GrpcSessionPeerView[],
  q: string,
  filter: "all" | "online" | "offline",
): GrpcSessionPeerView[] {
  let out = peers;
  if (filter === "online") {
    out = out.filter((p) => p.online);
  } else if (filter === "offline") {
    out = out.filter((p) => !p.online);
  }
  if (q) {
    out = out.filter((p) => p.client_public_key_b64.toLowerCase().includes(q));
  }
  return out;
}

function sortPeers(peers: GrpcSessionPeerView[], sort: string): GrpcSessionPeerView[] {
  const copy = [...peers];
  if (sort === "client_key") {
    copy.sort((a, b) => a.client_public_key_b64.localeCompare(b.client_public_key_b64));
  } else if (sort === "online_first") {
    copy.sort((a, b) => {
      if (a.online !== b.online) {
        return a.online ? -1 : 1;
      }
      return new Date(b.last_seen_at).getTime() - new Date(a.last_seen_at).getTime();
    });
  } else {
    copy.sort((a, b) => new Date(b.last_seen_at).getTime() - new Date(a.last_seen_at).getTime());
  }
  return copy;
}

function readToolbarState(): { q: string; filter: "all" | "online" | "offline"; sort: string } {
  const searchEl = document.getElementById("grpc-peers-search") as HTMLInputElement | null;
  const filterEl = document.getElementById("grpc-peers-filter") as HTMLSelectElement | null;
  const sortEl = document.getElementById("grpc-peers-sort") as HTMLSelectElement | null;
  const q = (searchEl?.value ?? "").trim().toLowerCase();
  const fv = filterEl?.value ?? "all";
  const filter: "all" | "online" | "offline" =
    fv === "online" || fv === "offline" ? fv : "all";
  return { q, filter, sort: sortEl?.value ?? "last_seen" };
}

function applyPeersToolbarView(): void {
  const peersTbody = document.getElementById("grpc-sessions-peers-tbody");
  const metaEl = document.getElementById("grpc-peers-meta");
  if (!peersTbody) {
    return;
  }
  const { q, filter, sort } = readToolbarState();
  const filtered = filterPeers(lastPeersSnapshot, q, filter);
  const sorted = sortPeers(filtered, sort);
  peersTbody.innerHTML = renderPeerRows(sorted);
  if (metaEl) {
    metaEl.textContent = t("status.peers.rowsShown", {
      shown: sorted.length,
      total: lastPeersSnapshot.length,
    });
  }
}

function ensurePeersToolbar(): void {
  if (peersToolbarBound) {
    return;
  }
  peersToolbarBound = true;
  const search = document.getElementById("grpc-peers-search") as HTMLInputElement | null;
  search?.addEventListener("input", () => {
    if (peerSearchDebounce != null) {
      clearTimeout(peerSearchDebounce);
    }
    peerSearchDebounce = setTimeout(() => {
      peerSearchDebounce = null;
      applyPeersToolbarView();
    }, 150);
  });
  document.getElementById("grpc-peers-filter")?.addEventListener("change", () => applyPeersToolbarView());
  document.getElementById("grpc-peers-sort")?.addEventListener("change", () => applyPeersToolbarView());
}

let statusCardBindings = false;
function ensureStatusCardBindings(): void {
  if (statusCardBindings) {
    return;
  }
  statusCardBindings = true;
  document.getElementById("btn-copy-status-json")?.addEventListener("click", async (ev) => {
    ev.preventDefault();
    ev.stopPropagation();
    if (!lastRawStatusCoreJson) {
      return;
    }
    try {
      await navigator.clipboard.writeText(lastRawStatusCoreJson);
    } catch {
      /* ignore */
    }
  });
  document.querySelector(".raw-json-summary")?.addEventListener("click", (ev) => {
    const t = ev.target as HTMLElement;
    if (t.closest("#btn-copy-status-json")) {
      ev.preventDefault();
    }
  });
}

function renderSystemStatusSuccess(core: StatusView): void {
  const grid = document.getElementById("status-summary-grid");
  const errP = document.getElementById("status-fetch-error");
  const pre = document.getElementById("status-json");
  if (!grid || !pre) {
    return;
  }
  errP && (errP.hidden = true);
  grid.hidden = false;
  const bv = badgeVariantForState(core.state);
  grid.innerHTML = `
    <dl class="status-dl">
      <div class="status-dl-row">
        <dt>${escapeHtml(t("status.field.version"))}</dt>
        <dd><code>${escapeHtml(core.current_version || "—")}</code></dd>
      </div>
      <div class="status-dl-row">
        <dt>${escapeHtml(t("status.field.state"))}</dt>
        <dd><span class="badge badge--${bv}">${escapeHtml(core.state || "—")}</span></dd>
      </div>
      <div class="status-dl-row">
        <dt>${escapeHtml(t("status.field.source"))}</dt>
        <dd><code class="source-pill">${escapeHtml(core.source || "—")}</code></dd>
      </div>
    </dl>`;
  const raw = JSON.stringify(core, null, 2);
  lastRawStatusCoreJson = raw;
  pre.innerHTML = highlightJsonForPre(raw);
}

function renderSystemStatusError(message: string): void {
  const grid = document.getElementById("status-summary-grid");
  const errP = document.getElementById("status-fetch-error");
  const pre = document.getElementById("status-json");
  if (!pre) {
    return;
  }
  lastRawStatusCoreJson = "";
  grid && (grid.hidden = true);
  if (errP) {
    errP.textContent = message;
    errP.hidden = false;
  }
  pre.textContent = message;
}

function updateKpiDeployFromStatus(core: StatusView): void {
  const stateEl = document.getElementById("kpi-deploy-state");
  const verEl = document.getElementById("kpi-deploy-version");
  const srcEl = document.getElementById("kpi-deploy-source");
  if (!stateEl || !verEl || !srcEl) {
    return;
  }
  const bv = badgeVariantForState(core.state);
  stateEl.innerHTML = `<span class="badge badge--${bv}">${escapeHtml(core.state || "—")}</span>`;
  verEl.textContent = core.current_version || "—";
  srcEl.textContent = core.source || "—";
}

function updateKpiStripLoadingDone(): void {
  document.getElementById("status-kpi-strip")?.removeAttribute("data-loading");
}

let grpcTcpDetailsListenerBound = false;

/** After first successful gRPC sessions render, avoid clearing DOM before fetch (stops 10s poll flicker). */
let grpcSessionsUiHydrated = false;

export async function refreshDashboard(): Promise<void> {
  ensureDisplayStreamForm();
  ensurePeersToolbar();
  ensureStatusCardBindings();

  const projectsEl = document.getElementById("projects")! as HTMLElement;
  const releasesEl = document.getElementById("releases")!;
  const historyEl = document.getElementById("history")!;

  const bundleWrap = document.getElementById("local-client-bundle") as HTMLElement | null;
  const bundleJson = document.getElementById("local-client-json") as HTMLElement | null;

  let statusOk = false;
  try {
    const data = await fetchStatus();
    const { local_client: lc, ...core } = data;
    statusOk = true;
    renderSystemStatusSuccess(core as StatusView);
    updateKpiDeployFromStatus(core as StatusView);
    const alertEl = document.getElementById("kpi-tile-alert");
    if (alertEl) {
      alertEl.hidden = true;
    }
    if (bundleWrap && bundleJson) {
      const bundle: Record<string, string> = {};
      if (lc?.token) {
        bundle.token = lc.token;
      }
      if (lc?.url) {
        bundle.url = lc.url;
      }
      if (lc?.pairing) {
        bundle.pairing = lc.pairing;
      }
      if (Object.keys(bundle).length > 0) {
        bundleWrap.hidden = false;
        bundleJson.textContent = JSON.stringify(bundle, null, 2);
      } else {
        bundleWrap.hidden = true;
        bundleJson.textContent = "";
      }
    }
  } catch (e) {
    renderSystemStatusError(formatErr(e));
    const stateEl = document.getElementById("kpi-deploy-state");
    const verEl = document.getElementById("kpi-deploy-version");
    const srcEl = document.getElementById("kpi-deploy-source");
    if (stateEl) {
      stateEl.innerHTML = `<span class="badge badge--crit">${escapeHtml(t("status.kpi.statusError"))}</span>`;
    }
    if (verEl) {
      verEl.textContent = "—";
    }
    if (srcEl) {
      srcEl.textContent = "—";
    }
    const alertEl = document.getElementById("kpi-tile-alert");
    const alertText = document.getElementById("kpi-alert-text");
    if (alertEl && alertText) {
      alertText.textContent = t("status.kpi.deployUnreachable");
      alertEl.hidden = false;
    }
    if (bundleWrap && bundleJson) {
      bundleWrap.hidden = true;
      bundleJson.textContent = "";
    }
  }

  try {
    const data = await fetchProjects();
    const rows = data.projects
      .map(
        (p) =>
          `<tr><td><code>${escapeHtml(p.id)}</code></td><td>${escapeHtml(p.deploy_root)}</td></tr>`,
      )
      .join("");
    projectsEl.innerHTML =
      `<table class="proj-table"><thead><tr><th>${escapeHtml(t("inv.table.id"))}</th><th>${escapeHtml(t("inv.table.deployRoot"))}</th></tr></thead><tbody>${rows}</tbody></table>`;
  } catch (e) {
    projectsEl.textContent = formatErr(e);
  }

  try {
    const data = await fetchReleases();
    releasesEl.textContent = JSON.stringify(data, null, 2);
  } catch (e) {
    releasesEl.textContent = formatErr(e);
  }

  try {
    const data = await fetchHistory();
    historyEl.textContent = JSON.stringify(data, null, 2);
  } catch (e) {
    historyEl.textContent = formatErr(e);
  }

  const benchEl = document.getElementById("grpc-sessions-benchmark");
  const peersTbody = document.getElementById("grpc-sessions-peers-tbody");
  const auditTbody = document.getElementById("grpc-sessions-audit-tbody");
  const errEl = document.getElementById("grpc-sessions-error");
  const tcpDetails = document.getElementById("grpc-sessions-tcp-details") as HTMLDetailsElement | null;
  const tcpTbody = document.getElementById("grpc-sessions-tcp-tbody");
  const tcpLoading = document.getElementById("grpc-sessions-tcp-loading");
  const tcpErrEl = document.getElementById("grpc-sessions-tcp-error");
  const kpiOnline = document.getElementById("kpi-clients-online");
  const kpiTotal = document.getElementById("kpi-clients-total");
  const kpiHint = document.getElementById("kpi-clients-hint");
  const kpiTraffic = document.getElementById("kpi-tile-traffic");
  const kpiAuditSummary = document.getElementById("kpi-audit-summary");

  if (peersTbody && auditTbody && errEl) {
    errEl.hidden = true;
    errEl.textContent = "";
    const resetTcpSection = !grpcSessionsUiHydrated;
    if (resetTcpSection) {
      peersTbody.innerHTML = "";
      auditTbody.innerHTML = "";
      if (tcpDetails) {
        tcpDetails.removeAttribute("data-loaded");
        tcpDetails.open = false;
      }
      if (tcpTbody) {
        tcpTbody.innerHTML = "";
      }
      if (tcpErrEl) {
        tcpErrEl.hidden = true;
        tcpErrEl.textContent = "";
      }
      if (tcpLoading) {
        tcpLoading.hidden = true;
      }
      if (benchEl) {
        benchEl.textContent = "";
        benchEl.hidden = true;
      }
    }
    try {
      const sess = await fetchGrpcSessions(120, { onlineSecs: GRPC_ONLINE_SECS });
      grpcSessionsUiHydrated = true;
      const { summary } = sess;
      const onlineCount = sess.peers.filter((p) => p.online).length;
      if (kpiOnline) {
        kpiOnline.textContent = String(onlineCount);
      }
      if (kpiTotal) {
        kpiTotal.textContent = t("status.kpi.clientsOfTotal", { total: sess.peers.length });
      }
      if (kpiHint) {
        kpiHint.textContent = t("status.kpi.clientsHint", { secs: GRPC_ONLINE_SECS });
      }
      if (kpiTraffic && kpiAuditSummary) {
        kpiAuditSummary.textContent = t("status.sessions.summaryLine", {
          total: summary.total_events,
          open: summary.tcp_open_total,
          closed: summary.tcp_close_total,
          estOpen: summary.estimated_open_tcp,
        });
        kpiTraffic.hidden = false;
      }
      const grpcAlert = document.getElementById("kpi-tile-alert");
      if (grpcAlert && statusOk) {
        grpcAlert.hidden = true;
      }
      if (benchEl) {
        if (sess.server_benchmark) {
          const b = sess.server_benchmark;
          benchEl.textContent = t("status.sessions.summaryBenchmark", {
            cpu: b.cpu_score,
            ram: b.ram_score,
            storage: b.storage_score,
            gpu: b.gpu_score != null && b.gpu_score !== undefined ? String(b.gpu_score) : "—",
            runAt: formatSessionsTime(b.run_at),
          });
          benchEl.hidden = false;
        } else {
          benchEl.textContent = "";
          benchEl.hidden = true;
        }
      }
      lastPeersSnapshot = sess.peers;
      applyPeersToolbarView();
      auditTbody.innerHTML = renderGrpcAuditRows(sess.recent);
      updateDisplayStreamPeerSelect(sess.peers);
    } catch (e) {
      if (!grpcSessionsUiHydrated) {
        lastPeersSnapshot = [];
        if (benchEl) {
          benchEl.textContent = "";
          benchEl.hidden = true;
        }
        peersTbody.innerHTML = "";
        auditTbody.innerHTML = "";
        updateDisplayStreamPeerSelect([]);
      }
      if (kpiTraffic) {
        kpiTraffic.hidden = true;
      }
      const msg =
        e instanceof ApiRequestError && e.status === 503
          ? t("status.sessions.unavailable")
          : formatErr(e);
      errEl.textContent = msg;
      errEl.hidden = false;
      const grpcAlert = document.getElementById("kpi-tile-alert");
      const alertText = document.getElementById("kpi-alert-text");
      if (grpcAlert && alertText && statusOk) {
        alertText.textContent = msg;
        grpcAlert.hidden = false;
      }
    }
  }

  if (tcpDetails && tcpTbody && tcpLoading && tcpErrEl && !grpcTcpDetailsListenerBound) {
    grpcTcpDetailsListenerBound = true;
    tcpDetails.addEventListener("toggle", async () => {
      if (!tcpDetails.open || tcpDetails.dataset.loaded === "1") {
        return;
      }
      tcpLoading.hidden = false;
      tcpErrEl.hidden = true;
      tcpErrEl.textContent = "";
      try {
        const full = await fetchGrpcSessions(120, {
          includeTcpAudit: true,
          onlineSecs: GRPC_ONLINE_SECS,
        });
        tcpDetails.dataset.loaded = "1";
        tcpTbody.innerHTML = renderGrpcAuditRows(full.recent);
      } catch (e) {
        tcpErrEl.textContent = formatErr(e);
        tcpErrEl.hidden = false;
      } finally {
        tcpLoading.hidden = true;
      }
    });
  }

  updateKpiStripLoadingDone();
}
