import React from "react";
import { ShieldAlert } from "lucide-react";

export interface ConfirmActionSpec {
  title: string;
  /** Short subtitle under the title (e.g. "Exits only from here"). */
  subtitle?: string;
  /** Full sentence(s) describing exactly what the action does — and what
   *  it deliberately does NOT do — so the operator confirms informed. */
  body: string;
  confirmLabel: string;
}

interface ConfirmActionModalProps {
  spec: ConfirmActionSpec | null;
  /** Async action — a rejection is reported via `onError`, not swallowed. */
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
  onError?: (message: string) => void;
}

/**
 * One confirmation pattern for every destructive command (flatten, halt,
 * emergency stop). Mirrors the header killswitch modal styling; the caller
 * supplies the wording so each surface describes its own blast radius.
 */
export const ConfirmActionModal: React.FC<ConfirmActionModalProps> = ({
  spec,
  onConfirm,
  onCancel,
  onError,
}) => {
  const [busy, setBusy] = React.useState(false);
  if (!spec) return null;

  const run = async () => {
    setBusy(true);
    try {
      await onConfirm();
    } catch (err) {
      onError?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
      onCancel();
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150">
      <div className="bg-surface border border-surface-border rounded-2xl p-6 max-w-md w-full shadow-2xl space-y-4 animate-in zoom-in-95 duration-150">
        <div className="flex items-center space-x-3">
          <div className="w-10 h-10 rounded-xl bg-loss-dim border border-loss/30 flex items-center justify-center text-loss">
            <ShieldAlert className="w-5 h-5" />
          </div>
          <div>
            <h3 className="text-base font-bold font-sans text-slate-900 leading-snug">{spec.title}</h3>
            {spec.subtitle && <p className="text-xs text-slate-500">{spec.subtitle}</p>}
          </div>
        </div>
        <p className="text-sm text-slate-600 leading-relaxed">{spec.body}</p>
        <div className="flex justify-end space-x-2.5 pt-3 border-t border-surface-border">
          <button onClick={onCancel} disabled={busy} className="btn-secondary px-2.5 py-2.5 text-sm">
            Cancel
          </button>
          <button
            onClick={() => void run()}
            disabled={busy}
            className="btn-danger px-2.5 py-2.5 text-sm font-bold shadow-sm disabled:opacity-50"
          >
            {busy ? "Working…" : spec.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
};
