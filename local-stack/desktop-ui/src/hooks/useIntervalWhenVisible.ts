import { useEffect, useRef } from "react";

/**
 * Runs `fn` every `intervalMs` while the document is visible and `enabled` is true.
 * Pauses when the tab is hidden or the browser reports the page as not visible.
 */
export function useIntervalWhenVisible(
  fn: () => void | Promise<void>,
  intervalMs: number,
  enabled = true,
): void {
  const fnRef = useRef(fn);
  fnRef.current = fn;

  useEffect(() => {
    if (!enabled || intervalMs <= 0) return;

    let id: ReturnType<typeof setInterval> | null = null;

    const tick = () => {
      if (document.hidden) return;
      void fnRef.current();
    };

    const start = () => {
      if (id != null) return;
      tick();
      id = setInterval(tick, intervalMs);
    };

    const stop = () => {
      if (id != null) {
        clearInterval(id);
        id = null;
      }
    };

    const onVis = () => {
      if (document.hidden) stop();
      else start();
    };

    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVis);
    return () => {
      document.removeEventListener("visibilitychange", onVis);
      stop();
    };
  }, [intervalMs, enabled]);
}
