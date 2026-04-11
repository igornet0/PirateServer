import { invoke } from "@tauri-apps/api/core";
import { Loader2, X } from "lucide-react";
import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-3 py-2 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

export const SERIES_RANGE_OPTIONS: { value: string; label: string }[] = [
  { value: "15m", label: "15 min" },
  { value: "1h", label: "1 hour" },
  { value: "24h", label: "1 day" },
  { value: "7d", label: "Week" },
];

type SeriesPoint = { ts_ms: number; value: number };
type SeriesResponse = { metric: string; step_ms: number; points: SeriesPoint[] };

function formatRateAxis(v: number): string {
  if (!Number.isFinite(v) || v < 0) return "0";
  if (v < 1024) return `${v.toFixed(0)} B/s`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} K/s`;
  return `${(v / 1024 / 1024).toFixed(2)} M/s`;
}

function toAlignedData(rx: SeriesResponse, tx: SeriesResponse): uPlot.AlignedData {
  const n = Math.min(rx.points.length, tx.points.length);
  const xs: number[] = [];
  const y1: number[] = [];
  const y2: number[] = [];
  for (let i = 0; i < n; i++) {
    const pr = rx.points[i];
    const pt = tx.points[i];
    xs.push(pr.ts_ms / 1000);
    y1.push(pr.value);
    y2.push(pt.value);
  }
  return [xs, y1, y2];
}

export function NetworkHostSeriesModal({
  open,
  onClose,
  baseUrl,
  ifaceLabel,
  onOpenDetail,
}: {
  open: boolean;
  onClose: () => void;
  baseUrl: string | null;
  ifaceLabel: string | null;
  onOpenDetail: () => void;
}) {
  const [range, setRange] = useState("1h");
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [payload, setPayload] = useState<string | null>(null);
  const chartRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);

  const load = useCallback(async () => {
    if (!baseUrl?.trim()) {
      setErr(
        "Control API base URL is not set. In Server connection, enter the HTTP control-api address (not the gRPC port 50051), e.g. http://192.168.0.30:8080",
      );
      return;
    }
    setLoading(true);
    setErr(null);
    setPayload(null);
    try {
      const raw = await invoke<string>("fetch_remote_host_stats_series", {
        baseUrl: baseUrl.trim(),
        range,
      });
      setPayload(raw);
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoading(false);
    }
  }, [baseUrl, range]);

  const pointCount = useMemo(() => {
    if (!payload) return 0;
    try {
      const p = JSON.parse(payload) as { net_rx: SeriesResponse; net_tx: SeriesResponse };
      return Math.min(p.net_rx.points.length, p.net_tx.points.length);
    } catch {
      return 0;
    }
  }, [payload]);

  useEffect(() => {
    if (!open) return;
    void load();
  }, [open, load]);

  useLayoutEffect(() => {
    if (!open || !payload || pointCount === 0) return;
    const el = chartRef.current;
    if (!el) return;
    let parsed: { net_rx: SeriesResponse; net_tx: SeriesResponse };
    try {
      parsed = JSON.parse(payload) as { net_rx: SeriesResponse; net_tx: SeriesResponse };
    } catch {
      return;
    }
    const data = toAlignedData(parsed.net_rx, parsed.net_tx);
    plotRef.current?.destroy();
    plotRef.current = null;
    if (data[0].length === 0) {
      return;
    }
    const w = el.clientWidth || 600;
    const h = 220;
    const opts: uPlot.Options = {
      width: w,
      height: h,
      scales: {
        x: { time: true },
        y: { range: (_, min, max) => [min < 0 ? 0 : min, max * 1.05 || 1] },
      },
      axes: [
        {},
        {
          label: "B/s",
          values: (_u, ticks) => ticks.map((v) => formatRateAxis(v as number)),
        },
      ],
      series: [
        {},
        {
          label: "RX (host)",
          stroke: "rgb(52, 211, 153)",
          width: 1.5,
        },
        {
          label: "TX (host)",
          stroke: "rgb(251, 191, 36)",
          width: 1.5,
        },
      ],
      legend: { show: true },
    };
    const plot = new uPlot(opts, data, el);
    plotRef.current = plot;
    return () => {
      plot.destroy();
      plotRef.current = null;
    };
  }, [open, payload, pointCount]);

  useEffect(() => {
    if (!open) {
      setPayload(null);
      setErr(null);
    }
  }, [open]);

  if (!open) return null;

  const title =
    ifaceLabel != null && ifaceLabel.length > 0
      ? `Network — ${ifaceLabel}`
      : "Network throughput";

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-labelledby="net-series-title"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div className="flex max-h-[90vh] w-full max-w-3xl flex-col rounded-2xl border border-white/10 bg-surface p-0 shadow-2xl">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-white/10 px-5 py-3">
          <div>
            <h3 id="net-series-title" className="text-lg font-semibold text-slate-100">
              {title}
            </h3>
            <p className="mt-1 text-xs text-slate-500">
              Uses HTTP control-api (not gRPC). History is host-wide. Set{" "}
              <strong className="text-slate-400">Control API (HTTP)</strong> in Server connection
              (e.g. <code className="text-amber-200/80">http://host:8080</code>, not port{" "}
              <code className="text-amber-200/80">50051</code>). Enable{" "}
              <code className="text-amber-200/80">CONTROL_API_HOST_STATS_SERIES=1</code>.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex flex-wrap gap-1 rounded-lg border border-white/10 bg-black/20 p-1">
              {SERIES_RANGE_OPTIONS.map((o) => (
                <button
                  key={o.value}
                  type="button"
                  onClick={() => setRange(o.value)}
                  className={`rounded-md px-2.5 py-1 text-xs font-medium ${
                    range === o.value
                      ? "bg-amber-700/80 text-white"
                      : "text-slate-400 hover:bg-white/5 hover:text-slate-200"
                  }`}
                >
                  {o.label}
                </button>
              ))}
            </div>
            <button
              type="button"
              onClick={() => onOpenDetail()}
              className={`${btnBase} border border-white/15 bg-white/5 text-xs text-slate-200 hover:bg-white/10`}
            >
              Detail (gRPC)
            </button>
            <button
              type="button"
              onClick={onClose}
              className={`${btnBase} border border-white/10 bg-white/5 p-2`}
              aria-label="Close"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
          {loading ? (
            <div className="flex items-center gap-2 text-slate-400">
              <Loader2 className="h-5 w-5 animate-spin" />
              Loading series…
            </div>
          ) : err ? (
            <p className="text-sm text-rose-300">{err}</p>
          ) : payload && pointCount === 0 ? (
            <p className="text-sm text-slate-500">
              No samples in this range yet (control-api must poll{" "}
              <code className="text-amber-200/70">/api/v1/host-stats</code> with series enabled).
            </p>
          ) : payload ? (
            <div ref={chartRef} className="w-full min-h-[220px]" />
          ) : (
            <p className="text-sm text-slate-500">No data.</p>
          )}
        </div>
      </div>
    </div>
  );
}
