import { useSyncExternalStore } from "react";

const QUERY = "(min-width: 1024px)";

export function useViewportMinLg(): boolean {
  return useSyncExternalStore(
    (onStoreChange) => {
      const m = window.matchMedia(QUERY);
      m.addEventListener("change", onStoreChange);
      return () => m.removeEventListener("change", onStoreChange);
    },
    () => window.matchMedia(QUERY).matches,
    () => true,
  );
}
