import { invoke } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import React, { useCallback, useEffect, useRef, useState } from "react";

function httpToWs(base: string): string {
  const u = base.trim();
  if (u.startsWith("https://")) return `wss://${u.slice(8)}`;
  if (u.startsWith("http://")) return `ws://${u.slice(7)}`;
  return u;
}

type Props = {
  controlBase: string;
  tr: (ru: string, en: string) => string;
  restartPending?: boolean;
};

export function HostTerminalPanel({ controlBase, tr, restartPending = false }: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  /** Incremented per connect; stale onclose/onerror must not clobber a newer socket. */
  const wsGenRef = useRef(0);
  const [status, setStatus] = useState<"idle" | "connecting" | "open" | "error">("idle");
  const [err, setErr] = useState<string | null>(null);

  const disconnect = useCallback(() => {
    wsGenRef.current += 1;
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
    setStatus("idle");
  }, []);

  const connect = useCallback(async () => {
    setErr(null);
    const base = controlBase.trim();
    if (!base) {
      setErr(
        tr(
          "Укажите Control API base URL на вкладке «Подключение».",
          "Set Control API base URL on the Connect tab.",
        ),
      );
      return;
    }
    disconnect();
    setStatus("connecting");
    try {
      await invoke("set_control_api_base", { url: base });
      const token = await invoke<string>("control_api_bearer_token");
      const url = `${httpToWs(base)}/api/v1/host-terminal/ws?access_token=${encodeURIComponent(token)}`;
      const ws = new WebSocket(url);
      ws.binaryType = "arraybuffer";
      const myGen = (wsGenRef.current += 1);
      wsRef.current = ws;

      ws.onopen = () => {
        if (wsRef.current !== ws || wsGenRef.current !== myGen) return;
        setStatus("open");
        termRef.current?.focus();
      };
      ws.onerror = () => {
        if (wsRef.current !== ws || wsGenRef.current !== myGen) return;
        setErr(
          tr(
            "Ошибка WebSocket: включите CONTROL_API_HOST_TERMINAL=1 на сервере и проверьте nginx (прокси WS для /api/…/ws).",
            "WebSocket error: enable CONTROL_API_HOST_TERMINAL=1 on the server and ensure nginx proxies WebSockets for /api/…/ws.",
          ),
        );
        setStatus("error");
      };
      ws.onclose = () => {
        if (wsGenRef.current !== myGen) return;
        wsRef.current = null;
        setStatus("idle");
      };
      ws.onmessage = (ev: MessageEvent) => {
        if (wsRef.current !== ws || wsGenRef.current !== myGen) return;
        const term = termRef.current;
        if (!term) return;
        if (ev.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(ev.data));
        } else if (typeof ev.data === "string") {
          term.write(ev.data);
        }
      };
    } catch (e) {
      setErr(String(e));
      setStatus("error");
    }
  }, [controlBase, disconnect, tr]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      theme: { background: "#0c0c0c", foreground: "#e8e8e8" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();
    termRef.current = term;

    term.onData((data) => {
      const ws = wsRef.current;
      if (ws?.readyState === WebSocket.OPEN) {
        ws.send(new TextEncoder().encode(data));
      }
    });

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        /* empty */
      }
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      disconnect();
      term.dispose();
      termRef.current = null;
    };
  }, [disconnect]);

  return (
    <div className="space-y-3">
      <p className="text-xs leading-relaxed text-slate-500">
        {tr(
          "Оболочка на хосте (пользователь ОС = процесс control-api). Требует переменных окружения на сервере; без TLS избегайте публичного доступа.",
          "Host shell (OS user is the control-api process). Requires server env vars; avoid public WAN without TLS.",
        )}
      </p>
      <p className="text-xs leading-relaxed text-slate-500">
        {tr(
          "«Отключить» закрывает только WebSocket-сессию терминала и не меняет JWT/login control-api.",
          "\"Disconnect\" only closes terminal WebSocket session and does not change control-api JWT/login.",
        )}
      </p>
      {restartPending ? (
        <p className="rounded-lg border border-amber-700/40 bg-amber-950/30 px-3 py-2 text-xs text-amber-100/95">
          {tr(
            "На сервере запланирован перезапуск control-api/deploy-server. Возможны короткие обрывы: дождитесь восстановления и переподключитесь.",
            "A control-api/deploy-server restart is scheduled on the server. Brief disconnects are expected: wait until it comes back, then reconnect.",
          )}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={status === "connecting" || status === "open"}
          onClick={() => void connect()}
          className="inline-flex items-center rounded-xl border border-red-800/45 bg-red-950/40 px-4 py-2 text-sm font-semibold text-orange-100 hover:bg-red-950/55 disabled:opacity-40"
        >
          {status === "connecting" ? "…" : tr("Подключить", "Connect")}
        </button>
        <button
          type="button"
          disabled={status !== "open" && status !== "connecting"}
          onClick={disconnect}
          className="inline-flex items-center rounded-xl border border-white/15 bg-white/5 px-4 py-2 text-sm font-semibold text-slate-200 hover:bg-white/10 disabled:opacity-40"
        >
          {tr("Отключить", "Disconnect")}
        </button>
      </div>
      {err ? <p className="text-sm text-rose-300">{err}</p> : null}
      <div
        ref={containerRef}
        className="h-[min(420px,calc(90vh-18rem))] min-h-[200px] w-full overflow-hidden rounded-xl border border-white/10 bg-black"
      />
    </div>
  );
}
