import {
  fetchGrpcSessions,
  fetchHistory,
  fetchProjects,
  fetchReleases,
  fetchStatus,
} from "../api/client.js";
import type { GrpcSessionEventView } from "../api/types.js";
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
  const t = s.trim();
  if (t.length <= max) {
    return t;
  }
  return `${t.slice(0, Math.max(0, max - 1))}…`;
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

function fmtUsagePct(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) {
    return "—";
  }
  return `${Math.round(v)}%`;
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

/** Matches control-api default `online_secs` for display copy. */
const GRPC_ONLINE_SECS = 120;

function renderGrpcAuditRows(rows: GrpcSessionEventView[]): string {
  return rows
    .map((r) => {
      const pk = r.client_public_key_b64?.trim() ?? "";
      const pkDisp = pk ? truncateCell(pk, 28) : "—";
      const det = truncateCell(r.detail ?? "", 48);
      return `<tr>
            <td>${escapeHtml(formatSessionsTime(r.created_at))}</td>
            <td><code>${escapeHtml(r.kind)}</code></td>
            <td><code>${escapeHtml(r.peer_ip || "—")}</code></td>
            <td>${escapeHtml(r.status)}</td>
            <td><code>${escapeHtml(r.grpc_method || "—")}</code></td>
            <td><code>${escapeHtml(pkDisp)}</code></td>
            <td>${escapeHtml(det)}</td>
          </tr>`;
    })
    .join("");
}

let grpcTcpDetailsListenerBound = false;

export async function refreshDashboard(): Promise<void> {
  const statusEl = document.getElementById("status")!;
  const projectsEl = document.getElementById("projects")! as HTMLElement;
  const releasesEl = document.getElementById("releases")!;
  const historyEl = document.getElementById("history")!;

  const bundleWrap = document.getElementById("local-client-bundle") as HTMLElement | null;
  const bundleJson = document.getElementById("local-client-json") as HTMLElement | null;

  try {
    const data = await fetchStatus();
    const { local_client: lc, ...core } = data;
    statusEl.textContent = JSON.stringify(core, null, 2);
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
    statusEl.textContent = formatErr(e);
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

  const sumEl = document.getElementById("grpc-sessions-summary");
  const sumPeersEl = document.getElementById("grpc-sessions-summary-peers");
  const benchEl = document.getElementById("grpc-sessions-benchmark");
  const peersTbody = document.getElementById("grpc-sessions-peers-tbody");
  const auditTbody = document.getElementById("grpc-sessions-audit-tbody");
  const errEl = document.getElementById("grpc-sessions-error");
  const tcpDetails = document.getElementById("grpc-sessions-tcp-details") as HTMLDetailsElement | null;
  const tcpTbody = document.getElementById("grpc-sessions-tcp-tbody");
  const tcpLoading = document.getElementById("grpc-sessions-tcp-loading");
  const tcpErrEl = document.getElementById("grpc-sessions-tcp-error");
  if (sumEl && sumPeersEl && peersTbody && auditTbody && errEl) {
    errEl.hidden = true;
    errEl.textContent = "";
    sumEl.textContent = t("loading");
    sumPeersEl.textContent = "";
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
    try {
      const sess = await fetchGrpcSessions(120, { onlineSecs: GRPC_ONLINE_SECS });
      const { summary } = sess;
      sumEl.textContent = t("status.sessions.summaryLine", {
        total: summary.total_events,
        open: summary.tcp_open_total,
        closed: summary.tcp_close_total,
        estOpen: summary.estimated_open_tcp,
      });
      const onlineCount = sess.peers.filter((p) => p.online).length;
      sumPeersEl.textContent = t("status.sessions.summaryPeersLine", {
        peerCount: sess.peers.length,
        onlineCount,
        onlineSecs: GRPC_ONLINE_SECS,
      });
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
      peersTbody.innerHTML = sess.peers
        .map((p) => {
          const pkDisp = truncateCell(p.client_public_key_b64, 36);
          const presence = p.online ? t("status.sessions.online") : t("status.sessions.offline");
          return `<tr>
            <td><code>${escapeHtml(pkDisp)}</code></td>
            <td><code>${escapeHtml(connectionKindLabel(p.connection_kind))}</code></td>
            <td>${escapeHtml(fmtUsagePct(p.last_cpu_percent))}</td>
            <td>${escapeHtml(fmtUsagePct(p.last_ram_percent))}</td>
            <td>${escapeHtml(fmtUsagePct(p.last_gpu_percent))}</td>
            <td>${escapeHtml(fmtBytesCompact(p.proxy_bytes_in_total))}</td>
            <td>${escapeHtml(fmtBytesCompact(p.proxy_bytes_out_total))}</td>
            <td><code>${escapeHtml(p.last_peer_ip || "—")}</code></td>
            <td><code>${escapeHtml(p.last_grpc_method || "—")}</code></td>
            <td>${escapeHtml(formatSessionsTime(p.last_seen_at))}</td>
            <td>${escapeHtml(presence)}</td>
          </tr>`;
        })
        .join("");
      auditTbody.innerHTML = renderGrpcAuditRows(sess.recent);
    } catch (e) {
      sumEl.textContent = "";
      sumPeersEl.textContent = "";
      if (benchEl) {
        benchEl.textContent = "";
        benchEl.hidden = true;
      }
      peersTbody.innerHTML = "";
      auditTbody.innerHTML = "";
      const msg =
        e instanceof ApiRequestError && e.status === 503
          ? t("status.sessions.unavailable")
          : formatErr(e);
      errEl.textContent = msg;
      errEl.hidden = false;
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
}
