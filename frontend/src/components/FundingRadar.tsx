import React, { useState } from "react";
import { TrendingUp, Clock, DollarSign, Send, CheckCircle2 } from "lucide-react";
import type { FundingOpportunityDto } from "../types";

interface FundingRadarProps {
  opportunities: FundingOpportunityDto[];
  symbol: string;
  onSymbolChange: (s: string) => void;
  tradeSize: number;
  onTradeSizeChange: (size: number) => void;
  onExecuteCarry?: (opp: FundingOpportunityDto) => Promise<void>;
  loading: boolean;
}

export const FundingRadar: React.FC<FundingRadarProps> = ({
  opportunities,
  symbol,
  onSymbolChange,
  tradeSize,
  onTradeSizeChange,
  onExecuteCarry,
}) => {
  const [holdingDays, setHoldingDays] = useState(14);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const symbols = ["ETH-USDC", "BTC-USDC", "SOL-USDC"];
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
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Top Banner & Configuration */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-4">
        <div>
          <h2 className="text-sm font-bold font-mono text-white flex items-center space-x-2">
            <TrendingUp className="w-4 h-4 text-profit" />
            <span>Delta-Neutral Perpetual Funding Carry Radar</span>
          </h2>
          <p className="text-xs text-slate-400 mt-1">
            Capture continuous funding rate spreads by going Long on low-funding venues and Short on high-funding venues.
          </p>
        </div>

        {/* Inputs */}
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center space-x-1.5 bg-surface-raised p-1 rounded-lg border border-surface-border">
            {symbols.map((s) => (
              <button
                key={s}
                onClick={() => onSymbolChange(s)}
                className={`px-3 py-1 text-xs font-mono rounded transition ${
                  symbol === s
                    ? "bg-profit text-background font-bold shadow-sm"
                    : "text-slate-300 hover:text-white"
                }`}
              >
                {s}
              </button>
            ))}
          </div>

          <div className="flex items-center space-x-1.5 bg-surface-raised p-1 rounded-lg border border-surface-border text-xs font-mono">
            <span className="px-2 text-slate-400">Size:</span>
            {quickSizes.map((qs) => (
              <button
                key={qs}
                onClick={() => onTradeSizeChange(qs)}
                className={`px-2.5 py-0.5 rounded transition ${
                  tradeSize === qs
                    ? "bg-accent/20 text-accent font-bold"
                    : "text-slate-400 hover:text-white"
                }`}
              >
                ${qs / 1000}k
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Evaluated Carry Opportunities Table */}
      <div className="bg-surface rounded-xl border border-surface-border p-5">
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-400 mb-4 flex items-center justify-between">
          <span>Ranked Delta-Neutral Carry Opportunities</span>
          <span className="text-slate-500 font-normal lowercase">compounding APY = (1 + r_hourly)^8760 - 1</span>
        </h3>

        {opportunities.length === 0 ? (
          <div className="text-center py-12 text-slate-500 font-mono text-xs border border-dashed border-surface-border rounded-lg">
            No funding rate spreads currently exceeding positive carry threshold.
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
            {opportunities.map((opp, idx) => (
              <div
                key={opp.id}
                className="bg-surface-raised border border-surface-border hover:border-slate-500 rounded-xl p-5 flex flex-col justify-between space-y-4 transition"
              >
                <div>
                  {/* Badge & Rank */}
                  <div className="flex items-center justify-between mb-3">
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono font-bold bg-profit/20 text-profit border border-profit/30">
                      RANK #{idx + 1}
                    </span>
                    <span className="text-xs font-mono font-bold text-accent">
                      {opp.net_apy_pct.toFixed(2)}% APY
                    </span>
                  </div>

                  {/* Pair Route */}
                  <div className="p-3 bg-surface rounded-lg border border-surface-border mb-3 font-mono text-xs space-y-2">
                    <div className="flex items-center justify-between">
                      <span className="text-slate-400">Long Leg:</span>
                      <span className="font-semibold text-white">{opp.long_venue}</span>
                    </div>
                    <div className="flex items-center justify-between">
                      <span className="text-slate-400">Short Leg:</span>
                      <span className="font-semibold text-white">{opp.short_venue}</span>
                    </div>
                    <div className="pt-1 border-t border-surface-border flex items-center justify-between text-profit">
                      <span>Net 8h Rate:</span>
                      <span className="font-bold">+{opp.net_rate_8h_pct.toFixed(4)}%</span>
                    </div>
                  </div>

                  {/* Key Metrics */}
                  <div className="grid grid-cols-2 gap-2 text-xs font-mono">
                    <div className="p-2 bg-surface rounded border border-surface-border">
                      <div className="text-[10px] text-slate-400 flex items-center space-x-1">
                        <Clock className="w-3 h-3 text-slate-400" />
                        <span>Fee Breakeven</span>
                      </div>
                      <div className="font-bold text-white mt-0.5">{opp.break_even_hours.toFixed(1)} hrs</div>
                    </div>

                    <div className="p-2 bg-surface rounded border border-surface-border">
                      <div className="text-[10px] text-slate-400 flex items-center space-x-1">
                        <DollarSign className="w-3 h-3 text-profit" />
                        <span>Daily Income</span>
                      </div>
                      <div className="font-bold text-profit mt-0.5">+${opp.estimated_daily_income_usd.toFixed(2)}</div>
                    </div>
                  </div>
                </div>

                {/* Return Projection & Action Button */}
                <div className="pt-3 border-t border-surface-border flex items-center justify-between text-xs font-mono">
                  <div>
                    <span className="text-slate-400">Annualized APR: </span>
                    <span className="font-bold text-slate-200">{opp.net_apr_pct.toFixed(2)}%</span>
                  </div>

                  {lastExecuted === opp.id ? (
                    <span className="inline-flex items-center space-x-1 text-profit text-xs font-bold">
                      <CheckCircle2 className="w-3.5 h-3.5" />
                      <span>Carry Active</span>
                    </span>
                  ) : (
                    <button
                      onClick={() => handleExecute(opp)}
                      disabled={executingId === opp.id}
                      className="px-2.5 py-1 bg-profit/20 hover:bg-profit/30 border border-profit/40 text-profit rounded-lg text-xs font-mono font-semibold transition flex items-center space-x-1"
                    >
                      <Send className="w-3 h-3" />
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
      <div className="bg-surface rounded-xl border border-surface-border p-5">
        <h3 className="text-xs font-mono uppercase tracking-wider text-slate-400 mb-4 flex items-center justify-between">
          <span>Carry Yield Amortization Simulator</span>
          <span className="text-accent font-bold font-mono text-xs">{holdingDays} Days Target</span>
        </h3>

        <div className="space-y-4">
          <div>
            <input
              type="range"
              min="1"
              max="90"
              value={holdingDays}
              onChange={(e) => setHoldingDays(parseInt(e.target.value))}
              className="w-full h-2 bg-surface-raised rounded-lg appearance-none cursor-pointer accent-accent"
            />
            <div className="flex justify-between text-[10px] font-mono text-slate-500 mt-1">
              <span>1 Day</span>
              <span>14 Days</span>
              <span>30 Days</span>
              <span>60 Days</span>
              <span>90 Days</span>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-4 gap-4 pt-2">
            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border">
              <div className="text-[10px] font-mono text-slate-400 uppercase">Deployed Capital</div>
              <div className="text-lg font-bold font-mono text-white mt-1">
                ${tradeSize.toLocaleString()}
              </div>
            </div>

            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border">
              <div className="text-[10px] font-mono text-slate-400 uppercase">Gross Funding Collected</div>
              <div className="text-lg font-bold font-mono text-profit mt-1">
                +${((tradeSize * 0.00021 * 3) * holdingDays).toFixed(2)}
              </div>
            </div>

            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border">
              <div className="text-[10px] font-mono text-slate-400 uppercase">Roundtrip Trading Fees</div>
              <div className="text-lg font-bold font-mono text-loss mt-1">
                -${(tradeSize * 0.00035 * 2).toFixed(2)}
              </div>
            </div>

            <div className="p-4 bg-surface-raised rounded-xl border border-surface-border">
              <div className="text-[10px] font-mono text-slate-400 uppercase">Net Realized PnL</div>
              <div className="text-lg font-bold font-mono text-profit mt-1">
                +${(((tradeSize * 0.00021 * 3) * holdingDays) - (tradeSize * 0.00035 * 2)).toFixed(2)}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
