import React, { useState, useEffect, useRef } from "react";
import { SlidersHorizontal, X } from "lucide-react";
import type { FundingOpportunityDto, ExecutionLogEntry } from "../types";
import type { PerpTicker } from "../types/tickers";
import { BotLifecycleControls } from "./BotLifecycleControls";
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
              <CarryDetailField label="APY" value={`${opp.net_apy_pct.toFixed(2)}%`} />
              <CarryDetailField label="Trade size" value={`$${size.toLocaleString()}`} />
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
                className="btn-primary !text-xs !py-1.5 !px-4 font-sans cursor-pointer"
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

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  availableTickers: _availableTickers,
  symbol,
  tradeSize,
  minAprPct = 8.0,
  onExecuteCarry,
  executionLogs: _executionLogs = [],
  onOpenConfig,
  isPaused = false,
  hasApiConnectivity = false,
}) => {
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const [detailOppId, setDetailOppId] = useState<string | null>(null);

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
    <div className="p-4 space-y-4 overflow-y-auto h-full">
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
            className="btn-secondary !text-xs !py-1.5 !px-3 font-sans cursor-pointer gap-2"
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
        <div className="flex items-center justify-between gap-3 mb-3 shrink-0">
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
                        className="btn-primary !text-[10px] !py-1 !px-2 font-sans cursor-pointer"
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
