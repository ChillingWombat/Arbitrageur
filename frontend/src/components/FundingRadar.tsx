import React, { useState, useMemo, useEffect, useRef } from "react";
import { SlidersHorizontal } from "lucide-react";
import type { FundingOpportunityDto, ExecutionLogEntry } from "../types";
import type { PerpTicker } from "../types/tickers";
import { formatCurrencyPnl } from "../types/tickers";
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
  availableTickers,
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
  const [selectedAsset, setSelectedAsset] = useState<string>("ALL");
  const [selectedOppId, setSelectedOppId] = useState<string | null>(null);

  // Bot lifecycle is strategy-wide for Funding Rate carry, not locked to a single ticker
  const bot = useBotLifecycle({ strategyId: "funding" });
  const lastAutoCarryExecRef = useRef<number>(0);

  // Derive strategy-wide execution stats from execution logs
  const strategyStats = useMemo(() => {
    let count = 0;
    let pnl = 0;

    for (const log of executionLogs) {
      const isFunding =
        log.strategy_id === "funding" ||
        log.trade_type?.toLowerCase().includes("funding");
      if (isFunding) {
        count += 1;
        pnl += log.net_profit || 0;
      }
    }
    return { count, pnl: Math.round(pnl * 100) / 100 };
  }, [executionLogs]);

  const monitoredCount = useMemo(() => {
    if (availableTickers && availableTickers.length > 0) {
      return availableTickers.length;
    }
    const distinct = new Set(opportunities.map((o) => o.symbol));
    return distinct.size;
  }, [availableTickers, opportunities]);

  const assetBase = (sym: string) => sym.split(/[-_/]/)[0]?.toUpperCase() ?? sym;

  const availableAssets = useMemo(() => {
    const set = new Set<string>();
    for (const o of opportunities) {
      set.add(assetBase(o.symbol));
    }
    if (availableTickers) {
      for (const t of availableTickers) {
        set.add(assetBase(t.symbol));
      }
    }
    return ["ALL", ...Array.from(set).sort()];
  }, [opportunities, availableTickers]);

  const filteredOpportunities = useMemo(() => {
    if (selectedAsset === "ALL") return opportunities;
    return opportunities.filter((o) => assetBase(o.symbol) === selectedAsset);
  }, [opportunities, selectedAsset]);

  // Selected or top opportunity for the amortization simulator
  const activeSimOpp = useMemo(() => {
    if (selectedOppId) {
      const found = opportunities.find((o) => o.id === selectedOppId);
      if (found) return found;
    }
    return filteredOpportunities[0] ?? opportunities[0] ?? null;
  }, [selectedOppId, opportunities, filteredOpportunities]);

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
          {/* Strategy-wide monitored scope + telemetry */}
          <div
            className="ribbon-pill font-bold text-slate-900"
            title="Monitoring all shared non-hidden tickers across active venues"
          >
            <span className="text-slate-400 font-normal">Monitored</span>
            <span>All Non-Hidden ({monitoredCount} Tickers)</span>
            <span
              className="text-xs px-1.5 py-0.2 rounded-sm bg-surface border border-surface-border font-medium text-slate-700 tabular-nums"
              title={`${strategyStats.count} strategy executions`}
            >
              {strategyStats.count} tx{strategyStats.count === 1 ? "" : "s"}
            </span>
            <span
              className={`text-xs px-1.5 py-0.2 rounded-sm font-semibold tabular-nums ${
                strategyStats.pnl > 0
                  ? "text-profit bg-profit-dim border border-profit-border"
                  : strategyStats.pnl < 0
                  ? "text-loss bg-loss-dim border border-loss-border"
                  : "text-slate-500 bg-surface border border-surface-border"
              }`}
              title={`Strategy Net PnL: ${formatCurrencyPnl(strategyStats.pnl)}`}
            >
              {formatCurrencyPnl(strategyStats.pnl)}
            </span>
          </div>

          <div
            className="ribbon-pill"
            title="Notional per leg — edit via Configure Strategy"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSize.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          <div
            className="ribbon-pill"
            title="Ignore opportunities below this net APR — edit via Configure Strategy"
          >
            <span className="text-slate-400">Min APR</span>
            <span className="font-bold text-slate-900 tabular-nums">{minAprPct}</span>
            <span className="text-slate-400">%</span>
          </div>

          <div
            className="ribbon-pill"
            title="Projected carry amortization duration — edit via Configure Strategy"
          >
            <span className="text-slate-400">Horizon</span>
            <span className="font-bold text-slate-900 tabular-nums">{holdingDays}</span>
            <span className="text-slate-400">Days</span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {/* Configure Strategy Button — universal params, applies to all funding carry */}
          <button
            type="button"
            onClick={() => onOpenConfig?.(symbol || "BTC-USDC", "funding")}
            className="btn-secondary !text-xs !py-1.5 !px-3 font-mono cursor-pointer"
            title="Configure Funding Rate strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
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

      {/* Evaluated Carry Opportunities Table */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-4">
          <div className="flex items-center space-x-2">
            <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
              Ranked Delta-Neutral Carry Opportunities
            </h3>
            <span className="text-xs font-mono text-slate-400">
              ({filteredOpportunities.length} of {opportunities.length} live)
            </span>
          </div>

          {/* In-tab asset filter tabs */}
          {availableAssets.length > 2 && (
            <div className="flex flex-wrap items-center gap-1.5">
              {availableAssets.map((asset) => (
                <button
                  key={asset}
                  type="button"
                  onClick={() => setSelectedAsset(asset)}
                  className={`px-2 py-0.5 rounded text-xs font-mono font-semibold transition cursor-pointer ${
                    selectedAsset === asset
                      ? "bg-accent text-white shadow-2xs"
                      : "bg-surface-raised hover:bg-slate-200 text-slate-600 border border-surface-border"
                  }`}
                >
                  {asset}
                </button>
              ))}
            </div>
          )}
        </div>

        {filteredOpportunities.length === 0 ? (
          <div className="border border-dashed border-surface-border rounded-xl">
            <EmptyState plain title={opportunities.length === 0 ? "No Opportunity Detected" : `No Opportunities for ${selectedAsset}`} />
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
            {filteredOpportunities.map((opp, idx) => {
              const isSelectedForSim = activeSimOpp?.id === opp.id;
              return (
                <div
                  key={opp.id}
                  onClick={() => setSelectedOppId(opp.id)}
                  className={`bg-surface border ${
                    isSelectedForSim
                      ? "border-accent ring-1 ring-accent/30 shadow-md"
                      : "border-surface-border hover:border-slate-300 shadow-card"
                  } rounded-xl p-4 flex flex-col justify-between space-y-3 transition cursor-pointer`}
                  title="Click to inspect in the Amortization Simulator below"
                >
                  <div>
                    {/* Badge, Ticker & Rank */}
                    <div className="flex items-center justify-between mb-3">
                      <div className="flex items-center space-x-2">
                        <span className="px-2 py-0.5 rounded-sm text-xs font-mono font-semibold bg-accent-dim text-accent border border-accent-line">
                          Rank #{idx + 1}
                        </span>
                        <span className="px-2 py-0.5 rounded-sm text-xs font-mono font-bold bg-slate-100 text-slate-800 border border-slate-200">
                          {opp.symbol}
                        </span>
                      </div>
                      <span className="text-xs font-mono font-bold text-accent tabular-nums">
                        {opp.net_apy_pct.toFixed(2)}% APY
                      </span>
                    </div>

                    {/* Pair Route */}
                    <div className="p-3 bg-surface-raised rounded-lg border border-surface-border mb-3 font-mono text-xs space-y-2">
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
                        <span className="font-bold tabular-nums">+{opp.net_rate_8h_pct.toFixed(4)}%</span>
                      </div>
                    </div>

                    {/* Key Metrics */}
                    <div className="grid grid-cols-2 gap-2 text-xs font-mono">
                      <div className="p-2.5 bg-surface-raised rounded-lg border border-surface-border">
                        <div className="text-xs text-slate-500">
                          Fee Breakeven
                        </div>
                        <div className="font-semibold text-slate-900 mt-0.5 tabular-nums">{opp.break_even_hours.toFixed(1)} hrs</div>
                      </div>

                      <div className="p-2.5 bg-surface-raised rounded-lg border border-surface-border">
                        <div className="text-xs text-slate-500">
                          Daily Income
                        </div>
                        <div className="font-semibold text-profit mt-0.5 tabular-nums">+${opp.estimated_daily_income_usd.toFixed(2)}</div>
                      </div>
                    </div>
                  </div>

                  {/* Return Projection & Action Button */}
                  <div className="pt-3 border-t border-surface-border flex items-center justify-between text-xs font-mono">
                    <div>
                      <span className="text-slate-500">Annualized APR: </span>
                      <span className="font-semibold text-slate-800 tabular-nums">{opp.net_apr_pct.toFixed(2)}%</span>
                    </div>

                    {lastExecuted === opp.id ? (
                      <span className="inline-flex items-center px-2 py-0.5 rounded-sm bg-profit-dim text-profit text-xs font-semibold">
                        Carry Active
                      </span>
                    ) : (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          handleExecute(opp);
                        }}
                        disabled={executingId === opp.id}
                        className="btn-primary !text-xs !py-1.5 !px-3 font-mono flex items-center cursor-pointer"
                      >
                        <span>{executingId === opp.id ? "Opening..." : "Open Carry"}</span>
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Interactive Holding Duration Yield Simulator */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card">
        <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 mb-4 flex items-center justify-between">
          <div className="flex items-center space-x-2">
            <span>Carry Yield Amortization Simulator</span>
            {activeSimOpp && (
              <span className="px-2 py-0.5 rounded-sm text-xs font-mono font-bold bg-slate-100 text-slate-800 border border-slate-200">
                {activeSimOpp.symbol} ({activeSimOpp.long_venue} / {activeSimOpp.short_venue})
              </span>
            )}
          </div>
          <span className="text-accent font-semibold font-mono text-xs tabular-nums">{holdingDays} Days Target</span>
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
              // Projections use the active simulated opportunity's actual net rate and round-trip fees
              const best = activeSimOpp;
              const gross = best ? tradeSize * (best.net_rate_8h_pct / 100) * 3 * holdingDays : null;
              const fees = best ? tradeSize * (best.round_trip_fee_bps / 10000) : null;
              const net = gross !== null && fees !== null ? gross - fees : null;
              return (
                <>
                  <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-xs">
                    <div className="text-xs font-mono text-slate-500">Deployed Capital</div>
                    <div className="text-lg font-bold font-mono text-slate-900 mt-1 tabular-nums">
                      ${tradeSize.toLocaleString()}
                    </div>
                  </div>

                  <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-xs">
                    <div className="text-xs font-mono text-slate-500">Gross Funding Collected</div>
                    <div className="text-lg font-bold font-mono text-profit mt-1 tabular-nums">
                      {gross !== null ? `+$${gross.toFixed(2)}` : "—"}
                    </div>
                    {best && (
                      <div className="text-xs font-mono text-slate-400 mt-0.5 tabular-nums">
                        @ +{best.net_rate_8h_pct.toFixed(4)}%/8h ({best.long_venue}/{best.short_venue})
                      </div>
                    )}
                  </div>

                  <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-xs">
                    <div className="text-xs font-mono text-slate-500">Roundtrip Trading Fees</div>
                    <div className="text-lg font-bold font-mono text-loss mt-1 tabular-nums">
                      {fees !== null ? `-$${fees.toFixed(2)}` : "—"}
                    </div>
                    {best && (
                      <div className="text-xs font-mono text-slate-400 mt-0.5 tabular-nums">
                        {best.round_trip_fee_bps.toFixed(1)} bps round-trip
                      </div>
                    )}
                  </div>

                  <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-xs">
                    <div className="text-xs font-mono text-slate-500">Projected Net Carry</div>
                    <div className={`text-lg font-bold font-mono mt-1 tabular-nums ${net !== null && net >= 0 ? "text-profit" : "text-loss"}`}>
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
