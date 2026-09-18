import React, { useState, useEffect, useMemo, useRef } from "react";
import { SlidersHorizontal, X } from "lucide-react";
import type { FundingOpportunityDto, ExecutionLogEntry, OpenPositionDto } from "../types";
import type { PerpTicker } from "../types/tickers";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { EmptyState } from "./EmptyState";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  /** All shared non-hidden market tickers currently monitored */
  availableTickers?: PerpTicker[];
  /** Optional fallback / context symbol */
  symbol?: string | null;
  tradeSize: number;
  minAprPct?: number;
  onExecuteCarry?: (opp: FundingOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  /** Global open positions (App fan-out); the gadget filters strategy "funding". */
  openPositions?: OpenPositionDto[];
}

const CarryDetailField: React.FC<{ label: string; value: React.ReactNode; title?: string }> = ({
  label,
  value,
  title,
}) => (
  <div className="space-y-0.5" title={title}>
    <div className="text-[11px] font-sans text-slate-400 font-medium">{label}</div>
    <div className="text-sm font-bold font-sans text-slate-900 tabular-nums">{value}</div>
  </div>
);

interface CarryDetailModalProps {
  opp: FundingOpportunityDto;
  rank: number;
  executing: boolean;
  executed: boolean;
  onExecute: (opp: FundingOpportunityDto) => void;
  onClose: () => void;
}

const CarryDetailModal: React.FC<CarryDetailModalProps> = React.memo(
  ({ opp, rank, executing, executed, onExecute, onClose }) => {
    const autoH =
      Number.isFinite(opp.break_even_hours) && opp.break_even_hours > 0
        ? Math.max(0.1, Math.round((opp.break_even_hours / 24) * 10) / 10)
        : null;
    const size = opp.trade_size_usd;
    const gross =
      autoH != null ? (size * (opp.net_rate_8h_pct / 100) * 3 * autoH) : null;
    const fees = size * (opp.round_trip_fee_bps / 10000);
    const net = gross != null ? gross - fees : null;
    useEffect(() => {
      const handleKeyDown = (e: KeyboardEvent) => {
        if (e.key === "Escape") onClose();
      };
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }, [onClose]);
    return (
      <div
        className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        <div className="bg-surface border border-surface-border rounded-2xl w-[680px] max-w-[94vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent border border-accent-line">
                Rank #{rank}
              </span>
              <span className="text-sm font-bold font-sans text-slate-900">{opp.symbol}</span>
              <span className="text-xs font-sans text-slate-500">
                {opp.long_venue} / {opp.short_venue}
              </span>
              <span className="text-xs font-sans font-bold text-accent tabular-nums">
                {opp.net_apy_pct.toFixed(2)}% APY
              </span>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
              title="Close details (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-5 space-y-5">
            <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-3">
              <CarryDetailField label="Long leg" value={opp.long_venue} />
              <CarryDetailField label="Short leg" value={opp.short_venue} />
              <CarryDetailField
                label="Net 8h rate"
                value={`+${opp.net_rate_8h_pct.toFixed(4)}%`}
              />
              <CarryDetailField label="Annualized APR" value={`${opp.net_apr_pct.toFixed(2)}%`} />
              <CarryDetailField label="Compounded APY" value={`${opp.net_apy_pct.toFixed(2)}%`} />
              <CarryDetailField
                label="Win probability"
                value={opp.win_probability != null ? `${(opp.win_probability * 100).toFixed(0)}%` : "—"}
                title="Experimental fee-coverage heuristic over a 24h hold (informational only — never gates entries)"
              />
              <CarryDetailField
                label="Depth slope"
                value={opp.depth_slope_bps_per_1k != null ? `${opp.depth_slope_bps_per_1k.toFixed(2)} bps/$1k` : "—"}
                title="Marginal entry-cost gradient summed over both legs at trade size — how fast slippage grows per extra $1k (informational only)"
              />
              <CarryDetailField
                label="Type"
                value={opp.kind === "basis" ? "Basis (SPOT/PERP)" : "Carry (perp/perp)"}
                title="Basis = same-exchange long-spot / short-perp cash-and-carry; Carry = cross-exchange perp/perp funding"
              />
              {opp.kind === "basis" && (
                <CarryDetailField
                  label="Basis divergence"
                  value={opp.basis_bps != null ? `${opp.basis_bps >= 0 ? "+" : ""}${opp.basis_bps.toFixed(1)} bps` : "—"}
                  title="Signed live divergence (perp - spot) / spot. Positive = contango."
                />
              )}
              {opp.kind === "basis" && opp.spot_price != null && (
                <CarryDetailField
                  label="Spot ref"
                  value={`$${opp.spot_price.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
                />
              )}
              {opp.kind === "basis" && opp.perp_price != null && (
                <CarryDetailField
                  label="Perp ref"
                  value={`$${opp.perp_price.toLocaleString(undefined, { maximumFractionDigits: 2 })}`}
                />
              )}
              <CarryDetailField
                label="Opportunity capacity"
                value={
                  opp.optimal_size_usd != null && opp.optimal_size_usd > 0
                    ? `$${opp.optimal_size_usd.toLocaleString(undefined, { maximumFractionDigits: 0 })}`
                    : "—"
                }
                title="Depth-supported size from both legs' live orderbooks"
              />
              <CarryDetailField
                label="Entry spread"
                value={
                  opp.entry_spread_bps != null && opp.entry_spread_bps >= 0
                    ? `${opp.entry_spread_bps.toFixed(1)} bps`
                    : "—"
                }
                title="Measured long-ask vs short-bid cross at executable size"
              />
              <CarryDetailField label="Fee breakeven" value={`${opp.break_even_hours.toFixed(1)} hrs`} />
              <CarryDetailField label="Daily income" value={`+$${opp.estimated_daily_income_usd.toFixed(2)}`} />
              <CarryDetailField
                label="Round-trip fees"
                value={`-$${fees.toFixed(2)} (${opp.round_trip_fee_bps.toFixed(1)} bps)`}
              />
              <CarryDetailField
                label="Auto hold"
                value={autoH != null ? `${autoH} days` : "—"}
                title="Derived from this opportunity's fee break-even — not set manually"
              />
              <CarryDetailField
                label="Projected net carry"
                value={
                  net != null ? `${net >= 0 ? "+" : "-"}$${Math.abs(net).toFixed(2)}` : "—"
                }
                title="Projected carry over the auto hold at the current 8h rate, net of round-trip fees"
              />
            </div>
          </div>

          <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
            {executed ? (
              <span className="inline-flex items-center px-2 py-0.5 rounded-md bg-profit-dim text-profit text-xs font-semibold">
                Carry Active
              </span>
            ) : (
              <button
                type="button"
                onClick={() => onExecute(opp)}
                disabled={executing}
                className="btn-primary !text-xs !py-2.5 !px-2.5 font-sans cursor-pointer"
              >
                <span>{executing ? "Opening..." : "Open Carry"}</span>
              </button>
            )}
          </div>
        </div>
      </div>
    );
  },
);

interface PositionDetailModalProps {
  pos: OpenPositionDto;
  onClose: () => void;
}

/** Current-position detail: facts + net-PnL trend. Solid segment = realized
 *  (entry at -fees → now at unrealized mark, never fabricated); dashed
 *  segment = illustrative 24h run-rate projection, shown ONLY when the
 *  backend supplies expectedNetUsd. Marginal-PnL / funding-diff / spread
 *  trend graphs arrive with position telemetry (deferred). */
const PositionDetailModal: React.FC<PositionDetailModalProps> = React.memo(({ pos, onClose }) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);
  const fees = pos.feesUsd ?? 0;
  const nowPnl = pos.unrealizedPnlUsd ?? -fees;
  const entryPnl = -fees;
  const hasProjection = pos.expectedNetUsd != null && Number.isFinite(pos.expectedNetUsd);
  // Chart geometry: x in [t0, t1, t2], y auto-scaled with padding.
  const W = 600, H = 180, PAD = 28;
  const t0 = Date.parse(pos.openedAt);
  const t1 = Date.now();
  const t2 = t1 + 24 * 3600 * 1000;
  const t0safe = Number.isFinite(t0) ? t0 : t1 - 3600 * 1000;
  const yEnd = hasProjection ? nowPnl + (pos.expectedNetUsd as number) : nowPnl;
  const lo = Math.min(entryPnl, nowPnl, yEnd);
  const hi = Math.max(entryPnl, nowPnl, yEnd);
  const span = hi - lo > 1e-9 ? hi - lo : 1;
  const xPos = (t: number) => PAD + ((t - t0safe) / Math.max(1, t2 - t0safe)) * (W - 2 * PAD);
  const yPos = (v: number) => H - PAD - ((v - lo) / span) * (H - 2 * PAD);
  const zeroY = yPos(0);
  const fmtT = (t: number) =>
    new Date(t).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[680px] max-w-[94vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold bg-accent-dim text-accent border border-accent-line">
              Position
            </span>
            <span className="text-sm font-bold font-sans text-slate-900">{pos.symbol}</span>
            <span className="text-xs font-sans text-slate-500">{pos.strategyId}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
            title="Close details (ESC)"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-3">
            <CarryDetailField label="Strategy" value={pos.strategyId} />
            <CarryDetailField
              label="Legs"
              value={pos.venues.map((v) => `${v.venue} (${v.netQty >= 0 ? "+" : ""}${v.netQty})`).join(" / ") || "—"}
            />
            <CarryDetailField
              label="Notional"
              value={pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
            />
            <CarryDetailField label="Fees paid" value={`-$${Math.abs(fees).toFixed(2)}`} />
            <CarryDetailField
              label="Unrealized PnL"
              value={`${nowPnl >= 0 ? "+" : "-"}$${Math.abs(nowPnl).toFixed(2)}`}
            />
            <CarryDetailField
              label="Signals"
              value={String(pos.signalCount)}
              title="Signal executions still counted as open in this position"
            />
            <CarryDetailField label="Opened" value={Number.isFinite(t0) ? fmtT(t0) : pos.openedAt} />
          </div>
          <div>
            <div className="text-[11px] font-sans text-slate-400 font-medium mb-1.5">
              Net PnL since entry — solid: realized · dashed: illustrative 24h projection
            </div>
            <svg viewBox={`0 0 ${W} ${H}`} className="w-full rounded-lg border border-surface-border bg-surface-raised" role="img">
              <line x1={PAD} y1={zeroY} x2={W - PAD} y2={zeroY} stroke="#cbd5e1" strokeWidth="1" strokeDasharray="2 3" />
              <polyline
                points={`${xPos(t0safe)},${yPos(entryPnl)} ${xPos(t1)},${yPos(nowPnl)}`}
                fill="none"
                stroke={nowPnl >= 0 ? "#15803d" : "#b91c1c"}
                strokeWidth="2.5"
              />
              {hasProjection && (
                <polyline
                  points={`${xPos(t1)},${yPos(nowPnl)} ${xPos(t2)},${yPos(yEnd)}`}
                  fill="none"
                  stroke="#64748b"
                  strokeWidth="2"
                  strokeDasharray="6 4"
                />
              )}
              <circle cx={xPos(t0safe)} cy={yPos(entryPnl)} r="3.5" fill="#64748b" />
              <circle cx={xPos(t1)} cy={yPos(nowPnl)} r="3.5" fill={nowPnl >= 0 ? "#15803d" : "#b91c1c"} />
              <text x={PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">{fmtT(t0safe)}</text>
              <text x={W - PAD} y={H - 8} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif" textAnchor="end">now{hasProjection ? " → +24h" : ""}</text>
              <text x={PAD} y={14} fontSize="10" fill="#94a3b8" fontFamily="Inter, sans-serif">
                {`entry -$${Math.abs(entryPnl).toFixed(0)} fees · now ${nowPnl >= 0 ? "+" : "-"}$${Math.abs(nowPnl).toFixed(0)}`}
              </text>
            </svg>
            {!hasProjection && (
              <div className="text-[11px] font-sans text-slate-400 mt-1.5">
                No expected-net figure from the backend, so no projection is drawn — nothing fabricated.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
});

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  availableTickers: _availableTickers,
  symbol,
  tradeSize,
  minAprPct = 8.0,
  onExecuteCarry,
  executionLogs = [],
  onOpenConfig,
  isPaused = false,
  hasApiConnectivity = false,
  openPositions = [],
}) => {
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const [detailOppId, setDetailOppId] = useState<string | null>(null);
  const [dockTab, setDockTab] = useState<"position" | "history" | "logs">("history");
  const [detailPosKey, setDetailPosKey] = useState<string | null>(null);
  const fundingPositions = useMemo(
    () => openPositions.filter((p) => (p.strategyId ?? "").toLowerCase() === "funding"),
    [openPositions]
  );
  const fundingLogs = useMemo(
    () =>
      executionLogs.filter(
        (l) =>
          l.strategy_id === "funding" ||
          (l.trade_type ?? "").toLowerCase().includes("funding") ||
          (l.trade_type ?? "").toLowerCase().includes("carry")
      ),
    [executionLogs]
  );

  // Bot lifecycle is strategy-wide for Funding Rate carry, not locked to a single ticker
  const bot = useBotLifecycle({ strategyId: "funding" });
  const lastAutoCarryExecRef = useRef<number>(0);



  const handleExecute = async (opp: FundingOpportunityDto) => {
    if (!onExecuteCarry) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") return;
    setExecutingId(opp.id);
    try {
      await onExecuteCarry(opp);
      setLastExecuted(opp.id);
      setTimeout(() => setLastExecuted(null), 3000);
    } finally {
      setExecutingId(null);
    }
  };

  // Automated Execution Loop across all monitored tickers when user enables Auto Carry
  useEffect(() => {
    if (!bot.entriesAllowed || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoCarryExecRef.current < 8000) return; // 8s cooldown

    // Select top funding carry opportunity with positive net yield meeting hurdle
    const topOpp = opportunities.find((o) => o.net_apr_pct >= minAprPct);
    if (topOpp) {
      lastAutoCarryExecRef.current = now;
      handleExecute(topOpp);
    }
  }, [bot.entriesAllowed, executingId, opportunities, isPaused, minAprPct]);

  return (
    <div className="p-4 pr-5 space-y-4 overflow-y-auto h-full">
      {/* Strategy Ribbon — read-only params left, configurator + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          <div
            className="ribbon-pill"
            title="Notional per leg — edit via Configurator"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSize.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          <div
            className="ribbon-pill"
            title="Ignore opportunities below this net APR — edit via Configurator"
          >
            <span className="text-slate-400">Min APR</span>
            <span className="font-bold text-slate-900 tabular-nums">{minAprPct}</span>
            <span className="text-slate-400">%</span>
          </div>

        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {/* Configure Strategy Button — universal params, applies to all funding carry */}
          <button
            type="button"
            onClick={() => onOpenConfig?.(symbol || "BTC-USDC", "funding")}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Funding Rate strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configurator</span>
          </button>

          {/* Auto Carry Bot Switch */}
          <BotLifecycleControls
            phase={bot.phase}
            hasOpenPositions={bot.hasOpenPositions}
            disabled={isPaused || !hasApiConnectivity}
            onStart={bot.start}
            onPause={bot.pause}
            onReduceOnly={bot.reduceOnly}
            onStop={() => void bot.stop()}
          />
          {bot.lastError && (
            <span className="text-xs font-mono text-loss max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
        </div>
      </div>

      {/* Opportunity Scanner — fixed window, 5x3 compact cards, click for details */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card h-[640px] flex flex-col">
        <div className="flex items-center justify-between gap-3 mb-3 pb-3 border-b border-surface-border shrink-0">
          <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
            Opportunity Scanner
          </h3>
        </div>

        {opportunities.length === 0 ? (
          <div className="border border-dashed border-surface-border rounded-xl">
            <EmptyState plain title="No Opportunity Detected" />
          </div>
        ) : (
          <div className="flex-1 min-h-0 overflow-y-auto">
            <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-5 gap-3">
              {opportunities.map((opp, idx) => (
                <div
                  key={opp.id}
                  onClick={() => setDetailOppId(opp.id)}
                  className="bg-surface border border-surface-border hover:border-accent-line rounded-xl p-2.5 flex flex-col justify-between gap-2 transition cursor-pointer shadow-card"
                  title="Click for opportunity details"
                >
                  <div className="space-y-2">
                    <div className="flex items-center justify-between gap-1.5">
                      <div className="flex items-center gap-1 min-w-0">
                        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-semibold bg-accent-dim text-accent border border-accent-line shrink-0">
                          #{idx + 1}
                        </span>
                        {opp.kind === "basis" && (
                          <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-bold bg-profit-dim text-profit border border-profit/30 shrink-0" title="Same-exchange long-spot / short-perp cash-and-carry">
                            BASIS
                          </span>
                        )}
                        <span className="px-1.5 py-0.5 rounded-md text-[10px] font-sans font-bold bg-slate-100 text-slate-800 border border-slate-200 truncate">
                          {opp.symbol}
                        </span>
                      </div>
                      <span className="text-[10px] font-sans font-bold text-accent tabular-nums shrink-0">
                        {opp.net_apy_pct.toFixed(1)}%
                      </span>
                    </div>

                    <div className="px-2 py-1.5 bg-surface-raised rounded-lg border border-surface-border font-sans text-[10px] space-y-1">
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Long:</span>
                        <span className="font-semibold text-slate-900 truncate">{opp.long_venue}</span>
                      </div>
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Short:</span>
                        <span className="font-semibold text-slate-900 truncate">{opp.short_venue}</span>
                      </div>
                      <div className="pt-1 border-t border-surface-border flex items-center justify-between text-profit">
                        <span>8h:</span>
                        <span className="font-bold tabular-nums">+{opp.net_rate_8h_pct.toFixed(3)}%</span>
                      </div>
                      {opp.kind === "basis" && opp.basis_bps != null && (
                        <div className="flex items-center justify-between gap-1">
                          <span className="text-slate-500">Basis:</span>
                          <span className="font-semibold text-slate-900 tabular-nums">{opp.basis_bps >= 0 ? "+" : ""}{opp.basis_bps.toFixed(1)} bps</span>
                        </div>
                      )}
                      <div className="flex items-center justify-between gap-1">
                        <span className="text-slate-500">Win:</span>
                        <span className="font-semibold text-slate-900 tabular-nums" title="Experimental fee-coverage heuristic — informational only">{opp.win_probability != null ? `${(opp.win_probability * 100).toFixed(0)}%` : "—"}</span>
                      </div>
                    </div>

                    <div className="grid grid-cols-2 gap-1.5 font-sans text-[10px]">
                      <div className="px-1.5 py-1 bg-surface-raised rounded-lg border border-surface-border">
                        <div className="text-slate-500">Breakeven</div>
                        <div className="font-semibold text-slate-900 tabular-nums">{opp.break_even_hours.toFixed(1)}h</div>
                      </div>
                      <div className="px-1.5 py-1 bg-surface-raised rounded-lg border border-surface-border">
                        <div className="text-slate-500">Daily</div>
                        <div className="font-semibold text-profit tabular-nums">+${opp.estimated_daily_income_usd.toFixed(0)}</div>
                      </div>
                    </div>
                  </div>

                  <div className="pt-2 border-t border-surface-border flex items-center justify-between font-sans text-[10px]">
                    <span className="font-semibold text-slate-800 tabular-nums">{opp.net_apr_pct.toFixed(1)}% APR</span>
                    {lastExecuted === opp.id ? (
                      <span className="inline-flex items-center px-1.5 py-0.5 rounded-md bg-profit-dim text-profit text-[10px] font-semibold">
                        Active
                      </span>
                    ) : (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          handleExecute(opp);
                        }}
                        disabled={executingId === opp.id}
                        className="btn-primary !text-[10px] !py-2 !px-2 font-sans cursor-pointer"
                      >
                        <span>{executingId === opp.id ? "Opening..." : "Open Carry"}</span>
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
      {/* Bottom dock: Current Position vs Execution History vs Log Monitor */}
      <div className="bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden shrink-0">
        <div className="flex items-stretch border-b border-surface-border bg-slate-50/80 select-none">
          <div className="grid grid-cols-3 flex-1 divide-x divide-surface-border">
            <button
              type="button"
              onClick={() => setDockTab("position")}
              className={`relative h-9 flex items-center justify-center gap-2 px-4 text-[11px] font-sans transition select-none cursor-pointer ${
                dockTab === "position"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Current Position</span>
              {fundingPositions.length > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                {fundingPositions.length}
              </span>
              )}
              {dockTab === "position" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
            <button
              type="button"
              onClick={() => setDockTab("history")}
              className={`relative h-9 flex items-center justify-center gap-2 px-4 text-[11px] font-sans transition select-none cursor-pointer ${
                dockTab === "history"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Execution History</span>
              {fundingLogs.length > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                {fundingLogs.length}
              </span>
              )}
              {dockTab === "history" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
            <button
              type="button"
              onClick={() => setDockTab("logs")}
              className={`relative h-9 flex items-center justify-center gap-2 px-4 text-[11px] font-sans transition select-none cursor-pointer ${
                dockTab === "logs"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <span>Log Monitor</span>
              {dockTab === "logs" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
          </div>
        </div>
        <div className="p-3.5 max-h-96 overflow-y-auto">
          {dockTab === "position" ? (
            fundingPositions.length === 0 ? (
              <EmptyState
                plain
                title="No Open Funding Positions"
                hint="Carry/basis fills will aggregate here per symbol with live PnL. Native execution is currently disabled, so this stays empty until authenticated order submission lands."
              />
            ) : (
              <div className="divide-y divide-surface-border">
                {fundingPositions.map((pos) => {
                  const pnl = pos.unrealizedPnlUsd;
                  return (
                    <button
                      key={pos.key}
                      type="button"
                      onClick={() => setDetailPosKey(pos.key)}
                      className="w-full flex items-center justify-between gap-3 px-2 py-2.5 rounded-lg hover:bg-slate-50 transition cursor-pointer text-left"
                      title="Click for position details and PnL trend"
                    >
                      <div className="min-w-0">
                        <div className="text-xs font-sans font-bold text-slate-900 truncate">{pos.symbol}</div>
                        <div className="text-[11px] font-sans text-slate-500 truncate">
                          {pos.venues.map((v) => v.venue).join(" / ") || pos.strategyId}
                        </div>
                      </div>
                      <div className="text-right shrink-0">
                        <div className={`text-xs font-sans font-bold tabular-nums ${pnl != null && pnl < 0 ? "text-loss" : "text-profit"}`}>
                          {pnl != null ? `${pnl >= 0 ? "+" : "-"}$${Math.abs(pnl).toFixed(2)}` : "—"}
                        </div>
                        <div className="text-[11px] font-sans text-slate-500 tabular-nums">
                          {pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
                        </div>
                      </div>
                    </button>
                  );
                })}
              </div>
            )
          ) : dockTab === "history" ? (
            <TickerTransactionHistory symbol="" logs={fundingLogs} matchAllSymbols />
          ) : (
            <TickerLogMonitor
              symbol=""
              namespace="funding-carry"
              strategyName="Funding Carry"
              executionLogs={fundingLogs}
              matchAllSymbols
              isPaused={isPaused}
            />
          )}
        </div>
      </div>
      {(() => {
        const detailPos =
          detailPosKey != null ? fundingPositions.find((x) => x.key === detailPosKey) ?? null : null;
        if (!detailPos) return null;
        return <PositionDetailModal pos={detailPos} onClose={() => setDetailPosKey(null)} />;
      })()}
      {(() => {
        const detailOpp =
          detailOppId != null ? opportunities.find((o) => o.id === detailOppId) ?? null : null;
        if (!detailOpp) return null;
        return (
          <CarryDetailModal
            opp={detailOpp}
            rank={opportunities.findIndex((o) => o.id === detailOpp.id) + 1}
            executing={executingId === detailOpp.id}
            executed={lastExecuted === detailOpp.id}
            onExecute={handleExecute}
            onClose={() => setDetailOppId(null)}
          />
        );
      })()}

    </div>
  );
};
