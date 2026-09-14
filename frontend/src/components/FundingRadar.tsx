import React, { useState, useMemo, useEffect, useRef } from "react";
import { SlidersHorizontal } from "lucide-react";
import type { FundingOpportunityDto, ExecutionLogEntry } from "../types";
import { formatCurrencyPnl } from "../types/tickers";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  /** Selected ticker; null until the user explicitly picks one in the explorer. */
  symbol: string | null;
  tradeSize: number;
  minAprPct?: number;
  holdingDays?: number;
  onExecuteCarry?: (opp: FundingOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
}

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  symbol,
  tradeSize,
  minAprPct = 8.0,
  holdingDays = 14,
  onExecuteCarry,
  executionLogs = [],
  onOpenConfig,
  isPaused = false,
  hasApiConnectivity = false,
}) => {
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  // Bot lifecycle scoped to this strategy + active ticker
  const bot = useBotLifecycle({ strategyId: "funding", symbol });
  const lastAutoCarryExecRef = useRef<number>(0);

  // Derive execution stats for the currently selected ticker
  const tickerStats = useMemo(() => {
    let count = 0;
    let pnl = 0;
    const symKey = (symbol ?? "").toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];

    for (const log of executionLogs) {
      if (!log.symbol) continue;
      const logKey = log.symbol.toUpperCase();
      const logBase = logKey.split(/[-_/]/)[0];
      if (logKey === symKey || logBase === baseKey) {
        count += 1;
        pnl += log.net_profit || 0;
      }
    }
    return { count, pnl: Math.round(pnl * 100) / 100 };
  }, [executionLogs, symbol]);

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

  // Automated Execution Loop when user manually enables Auto Carry
  useEffect(() => {
    if (!bot.entriesAllowed || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoCarryExecRef.current < 8000) return; // 8s cooldown

    // Select top funding carry opportunity with positive net yield
    const topOpp = opportunities.find((o) => o.symbol === symbol && o.net_apr_pct >= minAprPct);
    if (topOpp) {
      lastAutoCarryExecRef.current = now;
      handleExecute(topOpp);
    }
  }, [bot.entriesAllowed, executingId, opportunities, symbol, isPaused, minAprPct]);

  return (
    <div className="p-4 space-y-4 overflow-y-auto h-full">
      {/* Strategy Ribbon — read-only params left, configurator + toggle right */}
      <div className="px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-xs font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          {/* Active symbol + strategy telemetry */}
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md font-bold text-slate-900"
            title="Active ticker — select in the Tickers panel"
          >
            <span className="text-slate-400 font-normal">Active</span>
            <span>{symbol ?? "—"}</span>
            <span
              className="text-xs px-1.5 py-0.2 rounded-sm bg-surface border border-surface-border font-medium text-slate-700"
              title={`${tickerStats.count} strategy executions`}
            >
              {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
            </span>
            <span
              className={`text-xs px-1.5 py-0.2 rounded-sm font-semibold ${
                tickerStats.pnl > 0
                  ? "text-profit bg-profit-dim border border-profit-border"
                  : tickerStats.pnl < 0
                  ? "text-loss bg-loss-dim border border-loss-border"
                  : "text-slate-500 bg-surface border border-surface-border"
              }`}
              title={`Strategy Net PnL: ${formatCurrencyPnl(tickerStats.pnl)}`}
            >
              {formatCurrencyPnl(tickerStats.pnl)}
            </span>
          </div>

          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Notional per leg — edit via Configure Strategy"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900">{tradeSize.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Ignore opportunities below this net APR — edit via Configure Strategy"
          >
            <span className="text-slate-400">Min APR</span>
            <span className="font-bold text-slate-900">{minAprPct}</span>
            <span className="text-slate-400">%</span>
          </div>

          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Projected carry amortization duration — edit via Configure Strategy"
          >
            <span className="text-slate-400">Horizon</span>
            <span className="font-bold text-slate-900">{holdingDays}</span>
            <span className="text-slate-400">Days</span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {/* Configure Strategy Button */}
          <button
            type="button"
            onClick={() => symbol && onOpenConfig?.(symbol, "funding")}
            disabled={!symbol}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-md border border-accent-line/60 hover:border-accent transition shadow-xs cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
            title={symbol ? `Configure ${symbol} Funding Carry Parameters` : "Select a ticker in the explorer first"}
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
          </button>

          {/* Auto Carry Bot Switch (Green for ON, White for OFF) */}
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
            <span className="text-xs font-mono text-red-600 max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
        </div>
      </div>

      {/* Evaluated Carry Opportunities Table */}
      <div className="bg-surface rounded-lg border border-surface-border p-5 shadow-xs">
        <h3 className="text-xs font-bold font-mono tracking-wider text-slate-900 mb-4">
          Ranked Delta-Neutral Carry Opportunities
        </h3>

        {opportunities.length === 0 ? (
          <div className="text-center py-12 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-md">
            No funding rate spreads currently exceeding positive carry threshold.
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
            {opportunities.map((opp, idx) => (
              <div
                key={opp.id}
                className="bg-surface border border-surface-border hover:border-slate-300 rounded-lg p-5 flex flex-col justify-between space-y-4 transition shadow-xs"
              >
                <div>
                  {/* Badge & Rank */}
                  <div className="flex items-center justify-between mb-3">
                    <span className="px-2 py-0.5 rounded-sm text-xs font-mono font-semibold bg-accent-dim text-accent border border-accent-line">
                      Rank #{idx + 1}
                    </span>
                    <span className="text-xs font-mono font-bold text-accent">
                      {opp.net_apy_pct.toFixed(2)}% APY
                    </span>
                  </div>

                  {/* Pair Route */}
                  <div className="p-3 bg-surface-raised rounded-md border border-surface-border mb-3 font-mono text-xs space-y-2">
                    <div className="flex items-center justify-between">
                      <span className="text-slate-500">Long Leg:</span>
                      <span className="font-semibold text-slate-900">{opp.long_venue}</span>
                    </div>
                    <div className="flex items-center justify-between">
                      <span className="text-slate-500">Short Leg:</span>
                      <span className="font-semibold text-slate-900">{opp.short_venue}</span>
                    </div>
                    <div className="pt-1 border-t border-surface-border flex items-center justify-between text-profit">
                      <span>Net 8h Rate:</span>
                      <span className="font-bold">+{opp.net_rate_8h_pct.toFixed(4)}%</span>
                    </div>
                  </div>

                  {/* Key Metrics */}
                  <div className="grid grid-cols-2 gap-2 text-xs font-mono">
                    <div className="p-2 bg-surface-raised rounded-sm border border-surface-border">
                      <div className="text-xs text-slate-500">
                        Fee Breakeven
                      </div>
                      <div className="font-semibold text-slate-900 mt-0.5">{opp.break_even_hours.toFixed(1)} hrs</div>
                    </div>

                    <div className="p-2 bg-surface-raised rounded-sm border border-surface-border">
                      <div className="text-xs text-slate-500">
                        Daily Income
                      </div>
                      <div className="font-semibold text-profit mt-0.5">+${opp.estimated_daily_income_usd.toFixed(2)}</div>
                    </div>
                  </div>
                </div>

                {/* Return Projection & Action Button */}
                <div className="pt-3 border-t border-surface-border flex items-center justify-between text-xs font-mono">
                  <div>
                    <span className="text-slate-500">Annualized APR: </span>
                    <span className="font-semibold text-slate-800">{opp.net_apr_pct.toFixed(2)}%</span>
                  </div>

                  {lastExecuted === opp.id ? (
                    <span className="inline-flex items-center px-2 py-0.5 rounded-sm bg-profit-dim text-profit text-xs font-semibold">
                      Carry Active
                    </span>
                  ) : (
                    <button
                      onClick={() => handleExecute(opp)}
                      disabled={executingId === opp.id}
                      className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white rounded-md text-xs font-mono font-medium transition flex items-center shadow-xs"
                    >
                      <span>{executingId === opp.id ? "Opening..." : "Open Carry"}</span>
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Interactive Holding Duration Yield Simulator */}
      <div className="bg-surface rounded-lg border border-surface-border p-5 shadow-xs">
        <h3 className="text-xs font-bold font-mono tracking-wider text-slate-900 mb-4 flex items-center justify-between">
          <span>Carry Yield Amortization Simulator</span>
          <span className="text-accent font-semibold font-mono text-xs">{holdingDays} Days Target</span>
        </h3>

        <div className="space-y-4">
          <div title="Holding horizon — edit via Configure Strategy">
            <div className="relative w-full h-2 bg-slate-200 rounded-md">
              <div
                className="absolute left-0 top-0 h-2 bg-accent/60 rounded-md"
                style={{ width: `${Math.min(100, (holdingDays / 90) * 100)}%` }}
              />
              <div
                className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-3.5 h-3.5 bg-accent rounded-full border-2 border-white shadow-2xs"
                style={{ left: `${Math.min(100, (holdingDays / 90) * 100)}%` }}
              />
            </div>
            <div className="flex justify-between text-xs font-mono text-slate-400 mt-1">
              <span>1 Day</span>
              <span>14 Days</span>
              <span>30 Days</span>
              <span>60 Days</span>
              <span>90 Days</span>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 pt-2">
            {(() => {
              // Projections use the best live opportunity's actual net rate
              // and round-trip fees for the selected symbol — never canned.
              const best = opportunities.find((o) => o.symbol === symbol) ?? opportunities[0] ?? null;
              const gross = best ? tradeSize * (best.net_rate_8h_pct / 100) * 3 * holdingDays : null;
              const fees = best ? tradeSize * (best.round_trip_fee_bps / 10000) : null;
              const net = gross !== null && fees !== null ? gross - fees : null;
              return (
                <>
                  <div className="p-4 bg-surface-raised rounded-lg border border-surface-border shadow-2xs">
                    <div className="text-xs font-mono text-slate-500">Deployed Capital</div>
                    <div className="text-lg font-bold font-mono text-slate-900 mt-1">
                      ${tradeSize.toLocaleString()}
                    </div>
                  </div>

                  <div className="p-4 bg-surface-raised rounded-lg border border-surface-border shadow-2xs">
                    <div className="text-xs font-mono text-slate-500">Gross Funding Collected</div>
                    <div className="text-lg font-bold font-mono text-profit mt-1">
                      {gross !== null ? `+$${gross.toFixed(2)}` : "—"}
                    </div>
                    {best && (
                      <div className="text-xs font-mono text-slate-400 mt-0.5">
                        @ +{best.net_rate_8h_pct.toFixed(4)}%/8h ({best.long_venue}/{best.short_venue})
                      </div>
                    )}
                  </div>

                  <div className="p-4 bg-surface-raised rounded-lg border border-surface-border shadow-2xs">
                    <div className="text-xs font-mono text-slate-500">Roundtrip Trading Fees</div>
                    <div className="text-lg font-bold font-mono text-loss mt-1">
                      {fees !== null ? `-$${fees.toFixed(2)}` : "—"}
                    </div>
                    {best && (
                      <div className="text-xs font-mono text-slate-400 mt-0.5">
                        {best.round_trip_fee_bps.toFixed(1)} bps round-trip
                      </div>
                    )}
                  </div>

                  <div className="p-4 bg-surface-raised rounded-lg border border-surface-border shadow-2xs">
                    <div className="text-xs font-mono text-slate-500">Projected Net Carry</div>
                    <div className={`text-lg font-bold font-mono mt-1 ${net !== null && net >= 0 ? "text-profit" : "text-loss"}`}>
                      {net !== null ? `${net >= 0 ? "+" : "-"}$${Math.abs(net).toFixed(2)}` : "—"}
                    </div>
                    {!best && (
                      <div className="text-xs font-mono text-slate-400 mt-0.5">No Live Carry Opportunity</div>
                    )}
                  </div>
                </>
              );
            })()}
          </div>
        </div>
      </div>
    </div>
  );
};
