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

export interface BotStatCardsProps {
  stats: BotCycleStats | null;
  winRatePct?: number | null;
  exitsToday?: number | null;
  openExposureUsd?: number | null;
  feesPaidUsd?: number | null;
  scannedCount?: number | null;
  topEdgeBps?: number | null;
}

export const BotStatCards: React.FC<BotStatCardsProps> = ({
  stats,
  winRatePct,
  exitsToday,
  openExposureUsd,
  feesPaidUsd,
  scannedCount,
  topEdgeBps,
}) => {
  const unknown = stats == null || stats.unknown;
  const profitTitle =
    stats?.profitUnattributedReason ??
    (stats?.netProfitUsd == null
      ? "Bot cycle stats unavailable."
      : "Attributed realized PnL from closed radar fills, net of fees.");

  const hasExtra =
    winRatePct !== undefined ||
    feesPaidUsd !== undefined ||
    scannedCount !== undefined ||
    openExposureUsd !== undefined;

  return (
    <div className={`grid gap-3.5 shrink-0 ${hasExtra ? "grid-cols-2 sm:grid-cols-3 xl:grid-cols-6" : "grid-cols-3"}`}>
      <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card space-y-1 hover:border-slate-300 transition-all">
        <span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">Total Net Profit</span>
        <div
          className={`text-xl font-bold font-mono tracking-tight tabular-nums ${unknown || stats?.netProfitUsd == null ? "text-muted" : stats.netProfitUsd >= 0 ? "text-profit" : "text-loss"}`}
          title={profitTitle}
        >
          {unknown ? "—" : fmtMoney(stats?.netProfitUsd ?? null)}
        </div>
        <div className="text-xs font-mono tabular-nums text-muted" title={profitTitle}>
          {unknown ? "—" : fmtBps(stats?.netProfitBps ?? null)}
        </div>
      </div>

      {hasExtra && (
        <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card space-y-1 hover:border-slate-300 transition-all">
          <span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">Win Rate</span>
          <div
            className={`text-xl font-bold font-mono tracking-tight tabular-nums ${winRatePct == null ? "text-muted" : winRatePct >= 50 ? "text-profit" : "text-loss"}`}
            title="Percentage of closed cycle round trips that settled with net positive capture"
          >
            {winRatePct != null ? `${winRatePct.toFixed(1)}%` : "—"}
          </div>
          <div className="text-xs font-mono tabular-nums text-muted">
            {stats?.completedCycles != null && stats.completedCycles > 0 ? "settled cycles" : "no closed rounds"}
          </div>
        </div>
      )}

      <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card space-y-1 hover:border-slate-300 transition-all">
        <span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">Completed Cycles</span>
        <div
          className="text-xl font-bold font-mono tracking-tight tabular-nums text-slate-900"
          title="Entered-then-closed round trips, any size"
        >
          {unknown ? "—" : (stats?.completedCycles ?? 0)}
        </div>
        <div className="text-xs font-mono tabular-nums text-muted">
          {exitsToday != null && exitsToday > 0 ? `+${exitsToday} exits today` : "all-time closed"}
        </div>
      </div>

      <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card space-y-1 hover:border-slate-300 transition-all">
        <span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">Open Cycles</span>
        <div
          className="text-xl font-bold font-mono tracking-tight tabular-nums text-slate-900"
          title="Entered, not yet closed"
        >
          {unknown ? "—" : (stats?.openCycles ?? 0)}
        </div>
        <div className="text-xs font-mono tabular-nums text-muted">
          {openExposureUsd != null && openExposureUsd > 0
            ? `$${openExposureUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })} notional`
            : "0 open exposure"}
        </div>
      </div>

      {hasExtra && (
        <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card space-y-1 hover:border-slate-300 transition-all">
          <span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">Fees Paid</span>
          <div
            className="text-xl font-bold font-mono tracking-tight tabular-nums text-slate-900"
            title="Cumulative venue taker fees paid across executed legs"
          >
            {feesPaidUsd != null ? `-$${Math.abs(feesPaidUsd).toFixed(2)}` : "—"}
          </div>
          <div className="text-xs font-mono tabular-nums text-muted">
            venue commissions
          </div>
        </div>
      )}

      {hasExtra && (
        <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card space-y-1 hover:border-slate-300 transition-all">
          <span className="text-[11px] font-sans tracking-wider uppercase text-slate-500 font-semibold">Active Signals</span>
          <div
            className="text-xl font-bold font-mono tracking-tight tabular-nums text-slate-900"
            title="Currently scanned spread opportunities across configured venues"
          >
            {scannedCount != null ? `${scannedCount} pairs` : "—"}
          </div>
          <div className="text-xs font-mono tabular-nums text-profit" title="Best available expected edge">
            {topEdgeBps != null && topEdgeBps > 0 ? `+${topEdgeBps.toFixed(1)} bps max` : "monitoring orderbooks"}
          </div>
        </div>
      )}
    </div>
  );
};
