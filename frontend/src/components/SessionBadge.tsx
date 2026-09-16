import React from "react";
import { sessionLabel } from "../services/session.ts";

interface SessionBadgeProps {
  sessionId: string | null | undefined;
  provisional?: boolean;
  allow?: number | null;
  sizeMult?: number | null;
}

const ALLOW_STYLE = (allow: number | null | undefined): string => {
  if (allow == null || !Number.isFinite(allow)) return "bg-slate-100 text-slate-500 border-slate-300";
  if (allow <= 0) return "bg-rose-50 text-rose-700 border-rose-300";
  if (allow < 1) return "bg-amber-50 text-amber-700 border-amber-300";
  return "bg-emerald-50 text-emerald-700 border-emerald-300";
};

export const SessionBadge: React.FC<SessionBadgeProps> = ({
  sessionId,
  provisional = false,
  allow = null,
  sizeMult = null,
}) => {
  const label = sessionLabel(sessionId);
  const title =
    sessionId == null
      ? "session unknown"
      : `session ${sessionId}${provisional ? " · provisional (neutral x1.0)" : ""}` +
        (allow != null && Number.isFinite(allow) ? ` · allow x${allow.toFixed(2)}` : "") +
        (sizeMult != null && Number.isFinite(sizeMult) ? ` · size x${sizeMult.toFixed(2)}` : "");
  return (
    <span
      className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded-sm border text-xs ${ALLOW_STYLE(allow)}`}
      title={title}
    >
      {label}
      {provisional && (
        <span className="inline-block w-1.5 h-1.5 rounded-full bg-current opacity-70" title="provisional" />
      )}
      {allow != null && Number.isFinite(allow) && (
        <span className="font-bold">x{allow.toFixed(1)}</span>
      )}
    </span>
  );
};

export default SessionBadge;
