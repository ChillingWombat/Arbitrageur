import React, { useCallback, useEffect, useRef, useState } from "react";
import { Check, Loader2, XCircle } from "lucide-react";

/**
 * The one save button. Every section renders the same control with the
 * same lifecycle: idle "Save Changes" → "Saving…" → "Saved!" (green) or
 * "Failed!" (red), auto-resetting to idle. Callers either drive `state`
 * themselves or use `useSaveButton().run(promise)` — both render through
 * this component so no section can drift.
 */

export type SaveState = "idle" | "saving" | "saved" | "failed";

const DEFAULT_SUCCESS_MS = 2_500;
const DEFAULT_FAIL_MS = 3_000;

interface UseSaveButtonOptions {
  /** How long "Saved!" shows before returning to idle. */
  successMs?: number;
  /** How long "Failed!" shows before returning to idle. */
  failMs?: number;
  /** Keep "Saved!" until `reset()` — for forms that re-dirty on edit. */
  persistSaved?: boolean;
}

/**
 * State machine for SaveButton. `run(fn)` flips to saving, awaits `fn`,
 * lands on saved/failed, then returns `true`/`false` so callers can chain
 * (close modal, clear drafts) without duplicating try/finally. `fn` may
 * throw freely — the failure state is handled here; callers that need the
 * error message should catch inside `fn` and rethrow.
 */
export function useSaveButton({
  successMs = DEFAULT_SUCCESS_MS,
  failMs = DEFAULT_FAIL_MS,
  persistSaved = false,
}: UseSaveButtonOptions = {}) {
  const [state, setState] = useState<SaveState>("idle");
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = useCallback(() => {
    if (timerRef.current != null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const reset = useCallback(() => {
    clearTimer();
    setState("idle");
  }, [clearTimer]);

  useEffect(() => clearTimer, [clearTimer]);

  const run = useCallback(
    async (fn: () => Promise<void> | void): Promise<boolean> => {
      clearTimer();
      setState("saving");
      try {
        await fn();
      } catch {
        setState("failed");
        timerRef.current = setTimeout(() => setState("idle"), failMs);
        return false;
      }
      setState("saved");
      if (!persistSaved) {
        timerRef.current = setTimeout(() => setState("idle"), successMs);
      }
      return true;
    },
    [clearTimer, successMs, failMs, persistSaved],
  );

  return { state, run, reset };
}

interface SaveButtonProps {
  state: SaveState;
  /** "submit" for form-driven saves; default "button". */
  type?: "button" | "submit";
  onClick?: () => void;
  disabled?: boolean;
  /** Extra classes (width overrides etc.) — state colors stay managed. */
  className?: string;
}

export const SaveButton: React.FC<SaveButtonProps> = ({
  state,
  type = "button",
  onClick,
  disabled,
  className = "",
}) => {
  const palette =
    state === "saved"
      ? "bg-profit hover:bg-profit"
      : state === "failed"
      ? "bg-loss hover:bg-loss"
      : "";
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled || state === "saving"}
      className={`btn-accent ${palette} ${className}`.trim()}
      aria-live="polite"
    >
      {state === "saving" ? (
        <>
          <Loader2 className="w-3.5 h-3.5 animate-spin" />
          <span>Saving…</span>
        </>
      ) : state === "saved" ? (
        <>
          <Check className="w-3.5 h-3.5" />
          <span>Saved!</span>
        </>
      ) : state === "failed" ? (
        <>
          <XCircle className="w-3.5 h-3.5" />
          <span>Failed!</span>
        </>
      ) : (
        <span>Save Changes</span>
      )}
    </button>
  );
};
