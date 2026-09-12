import React, { useState, useMemo, useEffect, useRef } from "react";
import { SlidersHorizontal } from "lucide-react";
import type { FundingOpportunityDto, ExecutionLogEntry } from "../types";
import { formatCurrencyPnl } from "../types/tickers";
import { SwitchToggle } from "./SwitchToggle";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  symbol: string;
  tradeSize: number;
  onTradeSizeChange: (size: number) => void;
  onExecuteCarry?: (opp: FundingOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
}

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  symbol,
  tradeSize,
  onTradeSizeChange,
  onExecuteCarry,
  executionLogs = [],
  onOpenConfig,
  isPaused = false,
}) => {
  const [holdingDays, setHoldingDays] = useState(14);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const [autoCarryEnabled, setAutoCarryEnabled] = useState(false);
  const lastAutoCarryExecRef = useRef<number>(0);

  // Derive execution stats for the currently selected ticker
  const tickerStats = useMemo(() => {
    let count = 0;
    let pnl = 0;
    const symKey = symbol.toUpperCase();
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

  const quickSizes = [10000, 25000, 50000, 100000];

  const handleExecute = async (opp: FundingOpportunityDto) => {
    if (!onExecuteCarry) return;
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
    if (!autoCarryEnabled || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoCarryExecRef.current < 8000) return; // 8s cooldown

    // Select top funding carry opportunity with positive net yield
    const topOpp = opportunities.find((o) => o.symbol === symbol && o.net_apr_pct > 0);
    if (topOpp) {
      lastAutoCarryExecRef.current = now;
      handleExecute(topOpp);
    }
  }, [autoCarryEnabled, executingId, opportunities, symbol, isPaused]);

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Top Banner & Configuration */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <h2 className="text-sm font-bold font-mono text-slate-900">
            Delta-Neutral Perpetual Funding Carry Radar
          </h2>
          <p className="text-xs text-slate-500 mt-1">
            Capture continuous funding rate spreads by going Long on low-funding venues and Short on high-funding venues.
          </p>
        </div>

        {/* Inputs */}
        <div className="flex flex-wrap items-center gap-3">
          {/* Auto Carry Bot Switch (Green for ON, White for OFF) */}
          <div className="flex items-center space-x-1.5 px-2.5 py-1 bg-surface-raised border border-surface-border rounded-lg shadow-2xs">
            <span className="text-xs font-mono font-medium text-slate-700">Auto Carry</span>
            <SwitchToggle
              checked={autoCarryEnabled}
              onChange={setAutoCarryEnabled}
              size="sm"
              showStatusText={true}
            />
          </div>

          <div className="flex items-center space-x-2">
            <span className="text-xs font-mono uppercase text-slate-500 font-semibold">Active:</span>
            <div className="text-xs font-mono font-bold text-slate-900 bg-surface-raised px-2.5 py-1.5 rounded-md border border-surface-border flex items-center space-x-2">
              <span>{symbol}</span>
              <span
                className="text-[10px] px-1.5 py-0.2 rounded bg-surface border border-surface-border font-medium text-slate-700"
                title={`${tickerStats.count} strategy executions`}
              >
                {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
              </span>
              <span
                className={`text-[10px] px-1.5 py-0.2 rounded font-semibold ${
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
          </div>

          <div className="flex items-center space-x-1.5 bg-surface-raised p-1 rounded-lg border border-surface-border text-xs font-mono">
            <span className="px-2 text-slate-500">Size:</span>
            {quickSizes.map((qs) => (
              <button
                key={qs}
                onClick={() => onTradeSizeChange(qs)}
                className={`px-2.5 py-0.5 rounded-md transition ${
                  tradeSize === qs
                    ? "bg-accent-dim text-accent border border-accent-line font-semibold"
                    : "text-slate-600 hover:text-slate-900"
                }`}
              >
                ${qs / 1000}k
              </button>
            ))}
          </div>

          {/* Configure Strategy Button */}
          <button
            type="button"
            onClick={() => onOpenConfig?.(symbol, "funding")}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-lg border border-accent-line/60 hover:border-accent transition shadow-xs cursor-pointer"
            title={`Configure ${symbol} Funding Carry Parameters`}
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
          </button>
        </div>
      </div>

      {/* Evaluated Carry Opportunities Table */}
      <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-500 mb-4 flex items-center justify-between">
          <span>Ranked Delta-Neutral Carry Opportunities</span>
          <span className="text-slate-400 font-normal lowercase">compounding APY = (1 + r_hourly)^8760 - 1</span>
        </h3>

        {opportunities.length === 0 ? (
          <div className="text-center py-12 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-lg">
            No funding rate spreads currently exceeding positive carry threshold.
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
            {opportunities.map((opp, idx) => (
              <div
                key={opp.id}
                className="bg-surface border border-surface-border hover:border-slate-300 rounded-xl p-5 flex flex-col justify-between space-y-4 transition shadow-xs"
              >
                <div>
                  {/* Badge & Rank */}
                  <div className="flex items-center justify-between mb-3">
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono font-semibold bg-accent-dim text-accent border border-accent-line">
                      Rank #{idx + 1}
                    </span>
                    <span className="text-xs font-mono font-bold text-accent">
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
                      <span className="font-bold">+{opp.net_rate_8h_pct.toFixed(4)}%</span>
                    </div>
                  </div>

                  {/* Key Metrics */}
                  <div className="grid grid-cols-2 gap-2 text-xs font-mono">
                    <div className="p-2 bg-surface-raised rounded border border-surface-border">
                      <div className="text-[10px] text-slate-500 uppercase">
                        Fee Breakeven
                      </div>
                      <div className="font-semibold text-slate-900 mt-0.5">{opp.break_even_hours.toFixed(1)} hrs</div>
                    </div>

                    <div className="p-2 bg-surface-raised rounded border border-surface-border">
                      <div className="text-[10px] text-slate-500 uppercase">
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
                    <span className="inline-flex items-center px-2 py-0.5 rounded bg-profit-dim text-profit text-xs font-semibold">
                      Carry Active
                    </span>
                  ) : (
                    <button
                      onClick={() => handleExecute(opp)}
                      disabled={executingId === opp.id}
                      className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white rounded-lg text-xs font-mono font-medium transition flex items-center shadow-xs"
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
      <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-500 mb-4 flex items-center justify-between">
          <span>Carry Yield Amortization Simulator</span>
          <span className="text-accent font-semibold font-mono text-xs">{holdingDays} Days Target</span>
        </h3>

        <div className="space-y-4">
          <div>
            <input
              type="range"
              min="1"
              max="90"
              value={holdingDays}
              onChange={(e) => setHoldingDays(parseInt(e.target.value))}
              className="w-full h-2 bg-slate-200 rounded-lg appearance-none cursor-pointer accent-accent"
            />
            <div className="flex justify-between text-[10px] font-mono text-slate-400 mt-1">
              <span>1 Day</span>
              <span>14 Days</span>
              <span>30 Days</span>
              <span>60 Days</span>
              <span>90 Days</span>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 pt-2">
            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-2xs">
              <div className="text-[10px] font-mono text-slate-500 uppercase">Deployed Capital</div>
              <div className="text-lg font-bold font-mono text-slate-900 mt-1">
                ${tradeSize.toLocaleString()}
              </div>
            </div>

            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-2xs">
              <div className="text-[10px] font-mono text-slate-500 uppercase">Gross Funding Collected</div>
              <div className="text-lg font-bold font-mono text-profit mt-1">
                +${((tradeSize * 0.00021 * 3) * holdingDays).toFixed(2)}
              </div>
            </div>

            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-2xs">
              <div className="text-[10px] font-mono text-slate-500 uppercase">Roundtrip Trading Fees</div>
              <div className="text-lg font-bold font-mono text-loss mt-1">
                -${(tradeSize * 0.00035 * 2).toFixed(2)}
              </div>
            </div>

            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border shadow-2xs">
              <div className="text-[10px] font-mono text-slate-500 uppercase">Projected Net Carry</div>
              {(() => {
                const net = ((tradeSize * 0.00021 * 3) * holdingDays) - (tradeSize * 0.00035 * 2);
                return (
                  <div className={`text-lg font-bold font-mono mt-1 ${net >= 0 ? "text-profit" : "text-loss"}`}>
                    {net >= 0 ? `+$${net.toFixed(2)}` : `-$${Math.abs(net).toFixed(2)}`}
                  </div>
                );
              })()}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
