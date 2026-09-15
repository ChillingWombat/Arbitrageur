import React from "react";
import type { LucideIcon } from "lucide-react";

interface EmptyStateProps {
  /** Optional small muted icon rendered above the title. */
  icon?: LucideIcon;
  /** Pulses the icon — for "collecting / warming up" states. */
  pulse?: boolean;
  /** Renders the title as a quiet grey line instead of a bold header —
   *  for routine "nothing found" reminders. */
  plain?: boolean;
  title: React.ReactNode;
  /** Optional muted explanation under the title. */
  hint?: React.ReactNode;
  /** Optional action buttons under the hint. */
  children?: React.ReactNode;
}

/**
 * Shared empty-state / reminder block — bold mono title with an optional
 * muted explanation. Used inside panels and full-width table cells so every
 * "nothing to show" surface reads identically.
 */
export const EmptyState: React.FC<EmptyStateProps> = ({
  icon: Icon,
  pulse = false,
  plain = false,
  title,
  hint,
  children,
}) => (
  <div className="py-10 text-center text-xs font-mono text-slate-600 px-4 space-y-2.5">
    {Icon && (
      <Icon
        className={`w-5 h-5 mx-auto text-slate-300 ${pulse ? "animate-pulse" : ""}`}
      />
    )}
    <div className={plain ? "text-slate-400" : "text-slate-800 font-bold text-sm"}>{title}</div>
    {hint && <div className="text-slate-500 leading-snug">{hint}</div>}
    {children}
  </div>
);
