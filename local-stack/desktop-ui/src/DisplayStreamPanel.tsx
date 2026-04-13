/**
 * Local consumer: HTTP ingest + preview of last JPEG from remote producer.
 */
import { invoke } from "@tauri-apps/api/core";
import { Copy, Loader2, MonitorPlay } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

export function DisplayStreamPanel() {
  const [token, setToken] = useState("");
  const [port, setPort] = useState<number | null>(null);
  const [base, setBase] = useState<string | null>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [allowReceive, setAllowReceive] = useState(false);
  const [allowSend, setAllowSend] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        const p = await invoke<[boolean, boolean]>("get_display_stream_prefs");
        setAllowReceive(p[0]);
        setAllowSend(p[1]);
      } catch {
        /* ignore */
      }
    })();
  }, []);

  const savePrefs = async (recv: boolean, send: boolean) => {
    try {
      await invoke("set_display_stream_prefs", { allow_receive: recv, allow_send: send });
      setAllowReceive(recv);
      setAllowSend(send);
    } catch {
      /* ignore */
    }
  };

  const refreshPreview = useCallback(() => {
    setTick((t) => t + 1);
  }, []);

  const startIngest = async () => {
    setLoading(true);
    setErr(null);
    try {
      const tok = token.trim() ? token.trim() : null;
      const p = await invoke<number>("start_display_ingest", { token: tok });
      setPort(p);
      const b = await invoke<string | null>("display_ingest_base");
      setBase(b);
      const du = await invoke<string>("display_ingest_export_consumer_config", {
        token: tok,
      });
      setDataUrl(du);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!base) return;
    const id = window.setInterval(() => refreshPreview(), 200);
    return () => window.clearInterval(id);
  }, [base, refreshPreview]);

  const lastSrc =
    base && tick >= 0 ? `${base}/last.jpg?t=${tick}` : undefined;

  const copyDataUrl = async () => {
    if (!dataUrl) return;
    try {
      await navigator.clipboard.writeText(dataUrl);
    } catch {
      /* ignore */
    }
  };

  return (
    <section
      className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card"
      aria-labelledby="display-stream-heading"
    >
      <h2
        id="display-stream-heading"
        className="flex items-center gap-2 text-lg font-semibold text-slate-100"
      >
        <MonitorPlay className="h-5 w-5 text-amber-400/90" />
        Remote display (receive)
      </h2>
      <p className="mt-2 text-sm text-slate-400">
        Start local ingest, then paste the exported data URL on the producer machine (
        <code className="text-amber-200/80">client display-stream run …</code>). Optional Bearer
        token for POST /ingest.
      </p>
      <div className="mt-4 flex flex-col gap-2 rounded-xl border border-white/10 bg-black/20 p-3 text-sm text-slate-300">
        <span className="font-medium text-slate-200">Permissions (stored locally)</span>
        <label className="flex cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            checked={allowReceive}
            onChange={(e) => void savePrefs(e.target.checked, allowSend)}
            className="rounded border-white/20"
          />
          Allow receiving remote display stream on this machine
        </label>
        <label className="flex cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            checked={allowSend}
            onChange={(e) => void savePrefs(allowReceive, e.target.checked)}
            className="rounded border-white/20"
          />
          Allow sending this desktop to a remote ingest URL (CLI producer)
        </label>
      </div>
      <div className="mt-4 flex flex-col gap-3 sm:flex-row sm:items-end">
        <label className="flex min-w-[12rem] flex-1 flex-col gap-1 text-xs text-slate-500">
          Bearer token (optional)
          <input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            autoComplete="off"
            className="rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 placeholder:text-slate-600 focus:border-amber-600/50 focus:outline-none"
            placeholder="empty = no auth"
          />
        </label>
        <button
          type="button"
          disabled={loading}
          onClick={() => void startIngest()}
          className={`${btnBase} shrink-0 bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110`}
        >
          {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
          Start ingest server
        </button>
      </div>
      {port != null ? (
        <p className="mt-3 font-mono text-sm text-emerald-300/90">
          Listening port: {port} — ingest POST{" "}
          <code className="text-amber-200/90">
            {base?.replace(/^http:/, "http:")}/ingest
          </code>
        </p>
      ) : null}
      {dataUrl ? (
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void copyDataUrl()}
            className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
          >
            <Copy className="h-4 w-4" />
            Copy consumer data URL
          </button>
        </div>
      ) : null}
      {err ? <p className="mt-2 text-sm text-rose-300">{err}</p> : null}
      {lastSrc ? (
        <div className="mt-4 overflow-hidden rounded-xl border border-white/10 bg-black/40">
          <img
            src={lastSrc}
            alt="Last frame from producer"
            className="max-h-[min(480px,50vh)] w-full object-contain"
            onLoad={() => {}}
            onError={() => {}}
          />
        </div>
      ) : (
        <p className="mt-4 text-sm text-slate-500">Preview appears after ingest starts and frames arrive.</p>
      )}
    </section>
  );
}
