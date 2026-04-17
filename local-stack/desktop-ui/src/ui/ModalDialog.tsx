import React, { type ReactNode, useRef } from "react";
import { useModalOverlay } from "../hooks/useModalOverlay";

type Props = {
  open: boolean;
  onClose: () => void;
  /** Tailwind z-index class, e.g. z-modal */
  zClassName?: string;
  /**
   * viewport = fixed inset-0 (whole window).
   * container = absolute inset-0 — use when portaled into a positioned workspace column so the sidebar stays outside the overlay.
   */
  overlay?: "viewport" | "container";
  closeOnBackdrop?: boolean;
  closeOnEscape?: boolean;
  onEscapeKey?: () => void;
  children: ReactNode;
  className?: string;
  /** Backdrop extra classes */
  backdropClassName?: string;
  /** Inner wrapper around children (focus trap root + width) */
  panelClassName?: string;
  role?: "dialog" | "alertdialog";
  "aria-labelledby"?: string;
  "aria-modal"?: boolean;
};

/**
 * Backdrop + panel; Escape closes; optional backdrop click. Multi-step / settings modals: closeOnBackdrop={false}.
 */
export function ModalDialog({
  open,
  onClose,
  zClassName = "z-modal",
  overlay = "viewport",
  closeOnBackdrop = true,
  closeOnEscape = true,
  onEscapeKey,
  children,
  className = "flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm",
  backdropClassName = "",
  panelClassName = "w-full max-w-lg",
  role = "dialog",
  "aria-labelledby": ariaLabelledBy,
  "aria-modal": ariaModal = true,
}: Props) {
  const panelRef = useRef<HTMLDivElement>(null);
  useModalOverlay({
    open,
    onClose,
    closeOnEscape: open && closeOnEscape,
    onEscapeKey,
    focusContainerRef: panelRef,
  });

  if (!open) return null;

  const positionClass = overlay === "container" ? "absolute inset-0 min-h-0" : "fixed inset-0";

  return (
    <div
      className={`${positionClass} ${zClassName} ${className} ${backdropClassName}`.trim()}
      role={role}
      aria-modal={ariaModal}
      aria-labelledby={ariaLabelledBy}
      onClick={
        closeOnBackdrop
          ? (e) => {
              if (e.target === e.currentTarget) onClose();
            }
          : undefined
      }
    >
      <div ref={panelRef} className={panelClassName}>
        {children}
      </div>
    </div>
  );
}
