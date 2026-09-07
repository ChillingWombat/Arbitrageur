import React, { useState, useMemo } from "react";
import type { FundingOpportunityDto, ExecutionLogEntry } from "../types";
import { PERP_TICKERS, formatCurrencyPnl } from "../types/tickers";
import { Percent } from "lucide-react";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  symbol: string;
  tradeSize: number;
  onTradeSizeChange: (size: number) => void;
  onExecuteCarry?: (opp: FundingOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
}

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  symbol,
  tradeSize,
  onTradeSizeChange,
  onExecuteCarry,
  executionLogs = [],
}) => {
  const [holdingDays, setHoldingDays] = useState(14);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);

  const activeTicker = PERP_TICKERS.find((t) => t.symbol === symbol) || {
    symbol,
    base: symbol.split("-")[0],
    quote: symbol.split("-")[1] || "USDC",
    name: symbol,
    venues: ["HL", "LGT", "VNT"],
    basePrice: 100,
  };

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

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-6xl select-none">
      {/* Top Banner & Configuration */}
      <div className="glass-panel p-5 rounded-2xl border border-white/[0.08] flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <div className="flex items-center space-x-2.5">
            <div className="w-8 h-8 rounded-lg bg-emerald-500/15 border border-emerald-500/30 flex items-center justify-center text-emerald-300 shadow-[0_0_12px_rgba(52,211,153,0.2)]">
              <Percent className="w-4 h-4" />
            </div>
            <h2 className="text-base font-bold font-mono text-white tracking-tight">
              Delta-Neutral Perpetual Funding Carry Radar
            </h2>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            Capture continuous funding rate spreads by going Long on low-funding venues and Short on high-funding venues.
          </p>
        </div>

        {/* Inputs */}
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center space-x-2">
            <span className="text-xs font-mono uppercase text-white/40 font-semibold">Active:</span>
            <div className="text-xs font-mono font-bold text-white bg-white/[0.04] px-3 py-1.5 rounded-xl border border-white/[0.08] flex items-center space-x-2">
              <span>{symbol}</span>
              <span className="flex space-x-1">
                {activeTicker.venues.map((v) => (
                  <span key={v} className="chip chip-info text-[9px] px-1 py-0.2">
                    {v}
                  </span>
                ))}
              </span>
              <span
                className="text-[10px] px-1.5 py-0.2 rounded bg-white/[0.04] border border-white/[0.06] font-medium text-white/60 tabular-nums"
                title={`${tickerStats.count} strategy executions`}
              >
                {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
              </span>
              <span
                className={`text-[10px] px-1.5 py-0.2 rounded font-semibold tabular-nums ${
                  tickerStats.pnl > 0
                    ? "chip chip-pos"
                    : tickerStats.pnl < 0
                    ? "chip chip-neg"
                    : "text-white/40 bg-white/[0.04] border border-white/[0.06]"
                }`}
                title={`Strategy Net PnL: ${formatCurrencyPnl(tickerStats.pnl)}`}
              >
                {formatCurrencyPnl(tickerStats.pnl)}
              </span>
            </div>
          </div>

          <div className="flex items-center space-x-1 bg-white/[0.04] p-1 rounded-xl border border-white/[0.08] text-xs font-mono">
            <span className="px-2 text-white/40">Size:</span>
            {quickSizes.map((qs) => (
              <button
                key={qs}
                onClick={() => onTradeSizeChange(qs)}
                className={`px-2.5 py-1 rounded-lg transition tabular-nums ${
                  tradeSize === qs
                    ? "bg-cyan-500/20 text-cyan-300 border border-cyan-500/30 font-semibold shadow-[0_0_10px_rgba(34,211,238,0.2)]"
                    : "text-white/60 hover:text-white/90 hover:bg-white/[0.04]"
                }`}
              >
                ${qs / 1000}k
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Evaluated Carry Opportunities Table */}
      <div className="glass-panel rounded-2xl border border-white/[0.08] p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-white/40 mb-4 flex items-center justify-between">
          <span className="text-white/90 font-bold">Ranked Delta-Neutral Carry Opportunities</span>
          <span className="text-white/40 font-normal lowercase">compounding APY = (1 + r_hourly)^8760 - 1</span>
        </h3>

        {opportunities.length === 0 ? (
          <div className="text-center py-12 text-white/40 font-mono text-xs border border-dashed border-white/[0.08] rounded-xl">
            No funding rate spreads currently exceeding positive carry threshold.
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
            {opportunities.map((opp, idx) => (
              <div
                key={opp.id}
                className="glass-panel border border-white/[0.08] hover:border-cyan-500/40 rounded-2xl p-5 flex flex-col justify-between space-y-4 transition duration-200 shadow-xs"
              >
                <div>
                  {/* Badge & Rank */}
                  <div className="flex items-center justify-between mb-3">
                    <span className="chip chip-info text-[10px] font-mono font-semibold">
                      Rank #{idx + 1}
                    </span>
                    <span className="text-sm font-mono font-bold text-cyan-300 tabular-nums">
                      {opp.net_apy_pct.toFixed(2)}% APY
                    </span>
                  </div>

                  {/* Pair Route */}
                  <div className="p-3 bg-white/[0.03] rounded-xl border border-white/[0.06] mb-3 font-mono text-xs space-y-2">
                    <div className="flex items-center justify-between">
                      <span className="text-white/50">Long Leg:</span>
                      <span className="font-semibold text-white">{opp.long_venue}</span>
                    </div>
                    <div className="flex items-center justify-between">
                      <span className="text-white/50">Short Leg:</span>
                      <span className="font-semibold text-white">{opp.short_venue}</span>
                    </div>
                    <div className="pt-1 border-t border-white/[0.06] flex items-center justify-between text-emerald-400 tabular-nums">
                      <span>Net 8h Rate:</span>
                      <span className="font-bold">+{opp.net_rate_8h_pct.toFixed(4)}%</span>
                    </div>
                  </div>

                  {/* Key Metrics */}
                  <div className="grid grid-cols-2 gap-2 text-xs font-mono tabular-nums">
                    <div className="p-2.5 bg-white/[0.03] rounded-xl border border-white/[0.04]">
                      <div className="text-[10px] text-white/40 uppercase font-semibold">
                        Fee Breakeven
                      </div>
                      <div className="font-semibold text-white mt-0.5">{opp.break_even_hours.toFixed(1)} hrs</div>
                    </div>

                    <div className="p-2.5 bg-white/[0.03] rounded-xl border border-white/[0.04]">
                      <div className="text-[10px] text-white/40 uppercase font-semibold">
                        Daily Income
                      </div>
                      <div className="font-semibold text-emerald-400 mt-0.5">+${opp.estimated_daily_income_usd.toFixed(2)}</div>
                    </div>
                  </div>
                </div>

                {/* Return Projection & Action Button */}
                <div className="pt-3 border-t border-white/[0.06] flex items-center justify-between text-xs font-mono">
                  <div className="tabular-nums">
                    <span className="text-white/40">Annualized APR: </span>
                    <span className="font-semibold text-white">{opp.net_apr_pct.toFixed(2)}%</span>
                  </div>

                  {lastExecuted === opp.id ? (
                    <span className="chip chip-pos text-xs font-semibold">
                      Carry Active
                    </span>
                  ) : (
                    <button
                      onClick={() => handleExecute(opp)}
                      disabled={executingId === opp.id}
                      className="px-4 py-2 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white rounded-xl text-xs font-mono font-medium transition shadow-[0_0_14px_rgba(34,211,238,0.25)] flex items-center"
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
      <div className="glass-panel rounded-2xl border border-white/[0.08] p-5 shadow-xs">
        <h3 className="text-xs font-mono uppercase tracking-wider text-white/40 mb-4 flex items-center justify-between">
          <span className="text-white/90 font-bold">Carry Yield Amortization Simulator</span>
          <span className="text-cyan-300 font-semibold font-mono text-xs">{holdingDays} Days Target</span>
        </h3>

        <div className="space-y-4">
          <div>
            <input
              type="range"
              min="1"
              max="90"
              value={holdingDays}
              onChange={(e) => setHoldingDays(parseInt(e.target.value))}
              className="w-full h-2 bg-white/[0.1] rounded-lg appearance-none cursor-pointer accent-cyan-400"
            />
            <div className="flex justify-between text-[10px] font-mono text-white/40 mt-1">
              <span>1 Day</span>
              <span>14 Days</span>
              <span>30 Days</span>
              <span>60 Days</span>
              <span>90 Days</span>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 pt-2">
            <div className="p-4 glass-panel rounded-xl border border-white/[0.06] shadow-2xs tabular-nums">
              <div className="text-[10px] font-mono text-white/40 uppercase font-semibold">Deployed Capital</div>
              <div className="text-lg font-bold font-mono text-white mt-1">
                ${tradeSize.toLocaleString()}
              </div>
            </div>

            <div className="p-4 glass-panel rounded-xl border border-white/[0.06] shadow-2xs tabular-nums">
              <div className="text-[10px] font-mono text-white/40 uppercase font-semibold">Gross Funding Collected</div>
              <div className="text-lg font-bold font-mono text-emerald-400 mt-1">
                +${((tradeSize * 0.00021 * 3) * holdingDays).toFixed(2)}
              </div>
            </div>

            <div className="p-4 glass-panel rounded-xl border border-white/[0.06] shadow-2xs tabular-nums">
              <div className="text-[10px] font-mono text-white/40 uppercase font-semibold">Roundtrip Trading Fees</div>
              <div className="text-lg font-bold font-mono text-rose-400 mt-1">
                -${(tradeSize * 0.00035 * 2).toFixed(2)}
              </div>
            </div>

            <div className="p-4 glass-panel rounded-xl border border-white/[0.06] shadow-2xs tabular-nums">
              <div className="text-[10px] font-mono text-white/40 uppercase font-semibold">Net Realized PnL</div>
              {(() => {
                const net = ((tradeSize * 0.00021 * 3) * holdingDays) - (tradeSize * 0.00035 * 2);
                return (
                  <div className={`text-lg font-bold font-mono mt-1 ${net >= 0 ? "text-emerald-400" : "text-rose-400"}`}>
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
