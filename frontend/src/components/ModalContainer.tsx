import React, { useEffect } from "react";
import { X } from "lucide-react";

export interface ModalContainerProps {
  isOpen: boolean;
  onClose: () => void;
  title?: React.ReactNode;
  subtitle?: React.ReactNode;
  icon?: React.ReactNode;
  maxWidth?: "sm" | "md" | "lg" | "xl" | "2xl" | "3xl" | "4xl";
  children: React.ReactNode;
  className?: string;
  closeOnBackdropClick?: boolean;
}

const MAX_WIDTH_MAP = {
  sm: "max-w-sm",
  md: "max-w-md",
  lg: "max-w-lg",
  xl: "max-w-xl",
  "2xl": "max-w-2xl",
  "3xl": "max-w-3xl",
  "4xl": "max-w-4xl",
};

/**
 * Standardized ModalContainer adhering to the Arbitrageur Design System:
 * - Backdrop: .modal-backdrop (fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm)
 * - Card: .modal-card (bg-surface border border-surface-border rounded-2xl shadow-2xl)
 * - Standard ESC keyboard listener and dismiss affordances
 */
export const ModalContainer: React.FC<ModalContainerProps> = ({
  isOpen,
  onClose,
  title,
  subtitle,
  icon,
  maxWidth = "lg",
  children,
  className = "",
  closeOnBackdropClick = true,
}) => {
  useEffect(() => {
    if (!isOpen) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  const maxWidthClass = MAX_WIDTH_MAP[maxWidth] ?? "max-w-lg";

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (closeOnBackdropClick && e.target === e.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        className={`bg-surface border border-surface-border rounded-2xl shadow-2xl overflow-hidden w-full ${maxWidthClass} animate-in zoom-in-95 duration-150 ${className}`}
      >
        {(title || icon) && (
          <div className="flex items-center justify-between px-5 py-4 border-b border-surface-border bg-surface-raised/40">
            <div className="flex items-center space-x-3">
              {icon && (
                <div className="w-8 h-8 rounded-lg bg-surface border border-surface-border flex items-center justify-center shrink-0">
                  {icon}
                </div>
              )}
              <div>
                {title && (
                  <h3 className="text-sm font-sans font-bold text-slate-900 leading-snug">
                    {title}
                  </h3>
                )}
                {subtitle && (
                  <p className="text-xs text-slate-500 mt-0.5">{subtitle}</p>
                )}
              </div>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="p-1 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition cursor-pointer"
              aria-label="Close dialog"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        )}
        <div className="p-5">{children}</div>
      </div>
    </div>
  );
};
