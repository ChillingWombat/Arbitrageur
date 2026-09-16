import React from "react";
import type { BotCycleStats } from "../services/botCycleStats";

/**
 * Small bot stat gadget cards for the radar main page. A cycle is one
 * entered-then-closed round trip, any size (steps/grid stay the sizing unit
 * elsewhere and are never conflated here). Missing data renders as "—"
 * with the reason in the tooltip — never as a fabricated zero.
 */
function fmtMoney(v: number | null): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return v >= 0 ? `+$${v.toFixed(2)}` : `-$${Math.abs(v).toFixed(2)}`;
}

function fmtBps(v: number | null): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return `${v >= 0 ? "+" : "-"}${Math.abs(v).toFixed(1)} bps`;
}

export const BotStatCards: React.FC<{ stats: BotCycleStats | null }> = ({ stats }) => {
  const unknown = stats == null || stats.unknown;
  const profitTitle =
    stats?.profitUnattributedReason ??
    (stats?.netProfitUsd == null
      ? "Bot cycle stats unavailable."
      : "Attributed realized PnL from closed radar fills, net of fees.");
  return (
    <div className="grid grid-cols-3 gap-4 shrink-0">
      <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
        <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Bot Net Profit</span>
        <div
          className={`text-xl font-bold font-mono tracking-tight ${unknown || stats?.netProfitUsd == null ? "text-slate-400" : stats.netProfitUsd >= 0 ? "text-profit" : "text-loss"}`}
          title={profitTitle}
        >
          {unknown ? "—" : fmtMoney(stats?.netProfitUsd ?? null)}
        </div>
        <div className="text-xs font-mono text-slate-400" title={profitTitle}>
          {unknown ? "—" : fmtBps(stats?.netProfitBps ?? null)}
        </div>
      </div>
      <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
        <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Completed Cycles</span>
        <div
          className="text-xl font-bold font-mono tracking-tight text-slate-900"
          title="Entered-then-closed round trips, any size"
        >
          {unknown ? "—" : (stats?.completedCycles ?? 0)}
        </div>
        <div className="text-xs font-mono text-slate-400">entered → closed</div>
      </div>
      <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
        <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Open Cycles</span>
        <div
          className="text-xl font-bold font-mono tracking-tight text-slate-900"
          title="Entered, not yet closed"
        >
          {unknown ? "—" : (stats?.openCycles ?? 0)}
        </div>
        <div className="text-xs font-mono text-slate-400">entered, open</div>
      </div>
    </div>
  );
};
