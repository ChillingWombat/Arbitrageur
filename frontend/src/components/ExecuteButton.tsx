import React from "react";
import { Check, Loader2 } from "lucide-react";

/**
 * The one execute button. Every strategy's entry action renders the same
 * control: idle "Execute" → "Executing…" (spinner) while the dispatch is
 * in flight, and an optional "Executed" confirmation state for cards that
 * keep the button visible after a fill. Blocking/disabled semantics stay
 * with the caller (venue capability, size gates, open positions).
 */

interface ExecuteButtonProps {
  /** Dispatch in flight — shows spinner, disables the button. */
  executing?: boolean;
  /** Post-fill confirmation — green check, disabled. Omit to stay plain. */
  executed?: boolean;
  disabled?: boolean;
  onClick: () => void;
  /** Tooltip — pass the block reason / execute hint. */
  title?: string;
  className?: string;
}

export const ExecuteButton: React.FC<ExecuteButtonProps> = ({
  executing = false,
  executed = false,
  disabled = false,
  onClick,
  title,
  className = "",
}) => {
  if (executed) {
    return (
      <span
        className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-profit-dim text-profit border border-profit-border text-xs font-semibold ${className}`}
      >
        <Check className="w-3.5 h-3.5" />
        Executed
      </span>
    );
  }
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled || executing}
      title={title}
      className={`btn-primary font-sans ${className}`.trim()}
    >
      {executing ? (
        <>
          <Loader2 className="w-3.5 h-3.5 animate-spin" />
          <span>Executing…</span>
        </>
      ) : (
        <span>Execute</span>
      )}
    </button>
  );
};
