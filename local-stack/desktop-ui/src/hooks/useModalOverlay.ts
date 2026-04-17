import { type RefObject, useEffect, useRef } from "react";

/**
 * Modal overlay behavior: Escape to close, optional focus on first focusable inside container.
 * Backdrop click is handled per-modal (short dialogs often close; settings forms do not).
 */
export function useModalOverlay({
  open,
  onClose,
  closeOnEscape = true,
  /** If set, called on Escape instead of onClose (e.g. nested overlay dismiss first) */
  onEscapeKey,
  focusContainerRef,
  restoreFocusRef,
}: {
  open: boolean;
  onClose: () => void;
  closeOnEscape?: boolean;
  onEscapeKey?: () => void;
  /** Ref to the dialog panel; first [data-modal-initial-focus] or first button is focused when open */
  focusContainerRef: RefObject<HTMLElement | null>;
  /** Element to restore focus to when modal closes (optional) */
  restoreFocusRef?: RefObject<HTMLElement | null>;
}) {
  const prevActive = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open || !closeOnEscape) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (onEscapeKey) onEscapeKey();
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, closeOnEscape, onClose, onEscapeKey]);

  useEffect(() => {
    if (!open) {
      if (restoreFocusRef?.current) {
        restoreFocusRef.current.focus();
      } else if (prevActive.current && document.contains(prevActive.current)) {
        prevActive.current.focus();
      }
      prevActive.current = null;
      return;
    }
    prevActive.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const id = window.requestAnimationFrame(() => {
      const root = focusContainerRef.current;
      if (!root) return;
      const explicit = root.querySelector<HTMLElement>("[data-modal-initial-focus]");
      if (explicit) {
        explicit.focus();
        return;
      }
      const first =
        root.querySelector<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ) ?? null;
      first?.focus();
    });
    return () => window.cancelAnimationFrame(id);
  }, [open, focusContainerRef, restoreFocusRef]);
}
