/**
 * Local CLI probe (Docker, runtimes, nginx, DB clients) — read-only report.
 * Отчёт открывается модальным окном по кнопке «Локальное окружение».
 */
import { Check, ChevronDown, ChevronRight, Loader2, RefreshCw, Terminal, X, XCircle } from "lucide-react";
import React, { useMemo, useState } from "react";
import type { ToolchainItem, ToolchainReport } from "./toolchain-types";
import { useI18n } from "./i18n";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/70 disabled:pointer-events-none disabled:opacity-50";

function formatTime(ms: number): string {
  if (!ms) return "—";
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return "—";
  }
}

function versionLines(row: ToolchainItem): string[] {
  if (row.versions?.length) return row.versions;
  return [];
}

function versionCellSummary(row: ToolchainItem): string {
  const v = versionLines(row);
  if (v.length === 0) return "—";
  if (v.length === 1) return v[0]!;
  return `${v[0]!} (+${v.length - 1})`;
}

function reportStats(report: ToolchainReport | null): { ok: number; total: number } | null {
  if (!report?.items?.length) return null;
  const total = report.items.length;
  const ok = report.items.filter((i) => i.installed).length;
  return { ok, total };
}

export function LocalToolchainPanel({
  report,
  loading,
  err,
  onRefresh,
  defaultExpanded: _defaultExpanded,
}: {
  report: ToolchainReport | null;
  loading: boolean;
  err: string | null;
  onRefresh: () => void;
  /** @deprecated Раньше раскрывало панель inline; игнорируется, отчёт только в модалке. */
  defaultExpanded?: boolean;
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  void _defaultExpanded;
  const [modalOpen, setModalOpen] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const stats = useMemo(() => reportStats(report), [report]);

  const collapsedSubtitle = stats
    ? tr(`${stats.ok}/${stats.total} в строю`, `${stats.ok}/${stats.total} ready`)
    : loading
      ? t("auto.LocalToolchainPanel_tsx.1")
      : t("auto.LocalToolchainPanel_tsx.2");

  const modalBody = (
    <>
      <div className="shrink-0 border-b border-border-subtle bg-red-950/15 px-4 py-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 flex-1 items-start gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border border-red-900/45 bg-red-950/40 text-red-400/90">
              <Terminal className="h-5 w-5" aria-hidden />
            </div>
            <div className="min-w-0">
              <h2 id="toolchain-modal-title" className="font-display text-xl text-red-300/95">
                {t("auto.LocalToolchainPanel_tsx.3")}
              </h2>
              <p className="mt-1 max-w-prose text-xs leading-relaxed text-slate-500">
                {tr(
                  "Что нашлось в PATH: Docker, рантаймы и прочие инструменты. Первый запуск — при старте, дальше — по кнопке «Обновить». Автоустановки нет, только подсказки.",
                  "What was found in PATH: Docker, runtimes, and related tools. Initial probe runs at startup, then only via Refresh. No auto-install, only hints.",
                )}
              </p>
              {stats ? (
                <p className="mt-2 inline-flex flex-wrap items-center gap-2 rounded-md border border-red-900/30 bg-black/25 px-2 py-1 text-[11px] text-slate-400">
                  <span className="text-orange-200/90">
                    {tr(`${stats.ok}/${stats.total} на месте`, `${stats.ok}/${stats.total} available`)}
                  </span>
                  {report ? (
                    <span className="text-slate-600">· обновлено {formatTime(report.generatedAtMs)}</span>
                  ) : null}
                </p>
              ) : null}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={onRefresh}
              disabled={loading}
              className={`${btnSm} border border-red-900/45 bg-red-950/40 text-orange-100 hover:bg-red-950/60`}
            >
              {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
              {t("auto.LocalToolchainPanel_tsx.4")}
            </button>
            <button
              type="button"
              onClick={() => setModalOpen(false)}
              className="rounded-lg border border-border-subtle bg-panel-raised p-2 text-slate-400 transition hover:bg-white/10 hover:text-slate-200"
              title={t("auto.LocalToolchainPanel_tsx.5")}
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-4 [scrollbar-gutter:stable]">
        {err ? (
          <p className="mb-3 rounded-lg border border-red-900/50 bg-red-950/35 px-3 py-2 text-sm text-red-200/90">
            {err}
          </p>
        ) : null}

        {loading && !report ? (
          <div className="flex flex-col items-center justify-center gap-2 py-10 text-slate-500">
            <Loader2 className="h-8 w-8 animate-spin opacity-60" />
            <span className="text-xs">{t("auto.LocalToolchainPanel_tsx.6")}</span>
          </div>
        ) : (
          <ul className="space-y-2">
            {report?.items.map((row) => {
              const open = !!expanded[row.id];
              const vers = versionLines(row);
              return (
                <li
                  key={row.id}
                  className="overflow-hidden rounded-lg border border-border-subtle bg-black/25 transition hover:border-red-900/35"
                >
                  <div className="flex flex-wrap items-center gap-2 px-2.5 py-2 sm:gap-3">
                    <div className="min-w-0 flex-1 sm:grid sm:grid-cols-[minmax(8rem,1fr)_minmax(0,2fr)_auto] sm:items-center sm:gap-3">
                      <span className="block font-medium text-slate-200">{row.label}</span>
                      <code className="mt-0.5 block truncate font-mono text-[11px] text-orange-200/75 sm:mt-0">
                        {versionCellSummary(row)}
                      </code>
                      <div className="flex items-center gap-2 sm:justify-end">
                        {row.installed ? (
                          <span className="inline-flex items-center gap-1 rounded-full border border-orange-600/35 bg-orange-950/30 px-2 py-0.5 text-[11px] font-medium text-orange-200/95 shadow-[0_0_12px_rgba(234,88,12,0.12)]">
                            <Check className="h-3 w-3 shrink-0" /> {t("auto.LocalToolchainPanel_tsx.7")}
                          </span>
                        ) : (
                          <span className="inline-flex items-center gap-1 rounded-full border border-red-700/40 bg-red-950/40 px-2 py-0.5 text-[11px] font-medium text-red-300/90">
                            <XCircle className="h-3 w-3 shrink-0" /> {t("auto.LocalToolchainPanel_tsx.8")}
                          </span>
                        )}
                        <button
                          type="button"
                          className="rounded-md p-1.5 text-slate-500 hover:bg-white/10 hover:text-orange-200/90"
                          aria-expanded={open}
                          title={open ? t("auto.LocalToolchainPanel_tsx.9") : t("auto.LocalToolchainPanel_tsx.10")}
                          onClick={() =>
                            setExpanded((m) => ({
                              ...m,
                              [row.id]: !open,
                            }))
                          }
                        >
                          {open ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
                        </button>
                      </div>
                    </div>
                  </div>
                  {open ? (
                    <div className="space-y-3 border-t border-border-subtle bg-red-950/10 px-3 py-3 text-xs leading-relaxed text-slate-400">
                      {vers.length > 1 ? (
                        <div>
                          <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-wide text-slate-500">
                            {t("auto.LocalToolchainPanel_tsx.11")}
                          </p>
                          <ul className="space-y-1 font-mono text-[11px] text-orange-200/80">
                            {vers.map((line, i) => (
                              <li key={`${row.id}-${i}`} className="break-words rounded border border-border-subtle bg-black/30 px-2 py-1">
                                {line}
                              </li>
                            ))}
                          </ul>
                        </div>
                      ) : null}
                      <div>
                        <p className="mb-1 text-[10px] font-semibold uppercase tracking-wide text-slate-500">
                          {t("auto.LocalToolchainPanel_tsx.12")}
                        </p>
                        <p className="text-slate-400">{row.installHint}</p>
                      </div>
                    </div>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </>
  );

  return (
    <>
      <div className="overflow-hidden rounded-xl border border-border-subtle bg-panel shadow-card">
        <button
          type="button"
          onClick={() => setModalOpen(true)}
          className="flex w-full items-center gap-3 px-3 py-3 text-left transition hover:bg-red-950/25"
          aria-haspopup="dialog"
          aria-expanded={modalOpen}
          aria-controls="toolchain-modal-dialog"
        >
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border border-red-900/40 bg-red-950/35 text-red-400/90 shadow-[inset_0_0_12px_rgba(0,0,0,0.35)]">
            <Terminal className="h-5 w-5" aria-hidden />
          </div>
          <div className="min-w-0 flex-1">
            <p className="font-display text-lg leading-tight text-red-300/95">{t("auto.LocalToolchainPanel_tsx.13")}</p>
            <p className="mt-0.5 text-xs text-slate-500">
              {t("auto.LocalToolchainPanel_tsx.14")} · <span className="text-orange-200/80">{collapsedSubtitle}</span>
              {report ? (
                <span className="text-slate-600"> · {formatTime(report.generatedAtMs)}</span>
              ) : null}
            </p>
          </div>
        </button>
      </div>

      {modalOpen ? (
        <div
          className="fixed inset-0 z-modalToolchain flex items-center justify-center bg-black/75 p-4 backdrop-blur-sm"
          role="presentation"
          onClick={(e) => {
            if (e.target === e.currentTarget) setModalOpen(false);
          }}
        >
          <div
            id="toolchain-modal-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="toolchain-modal-title"
            className="flex max-h-[min(88vh,40rem)] w-full max-w-2xl min-h-0 flex-col overflow-hidden rounded-2xl border border-border-subtle bg-panel shadow-2xl shadow-red-950/40"
            onClick={(e) => e.stopPropagation()}
          >
            {modalBody}
          </div>
        </div>
      ) : null}
    </>
  );
}
