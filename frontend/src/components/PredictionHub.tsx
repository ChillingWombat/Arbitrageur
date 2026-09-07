import React, { useState } from "react";
import { Filter, Target } from "lucide-react";
import type { PredictionOpportunityDto } from "../types";

interface PredictionHubProps {
  opportunities: PredictionOpportunityDto[];
  onSearch: (market: string) => void;
  onExecutePrediction?: (opp: PredictionOpportunityDto) => Promise<void>;
  loading?: boolean;
}

export const PredictionHub: React.FC<PredictionHubProps> = ({
  opportunities,
  onSearch,
  onExecutePrediction,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    onSearch(searchTerm);
  };

  const handleExecute = async (opp: PredictionOpportunityDto) => {
    if (!onExecutePrediction) return;
    setExecutingId(opp.id);
    try {
      await onExecutePrediction(opp);
      setLastExecuted(opp.id);
      setTimeout(() => setLastExecuted(null), 3000);
    } finally {
      setExecutingId(null);
    }
  };

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-6xl select-none">
      {/* Header Info */}
      <div className="glass-panel p-5 rounded-2xl border border-white/[0.08] flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <div className="flex items-center space-x-2.5">
            <div className="w-8 h-8 rounded-lg bg-purple-500/15 border border-purple-500/30 flex items-center justify-center text-purple-300 shadow-[0_0_12px_rgba(168,85,247,0.2)]">
              <Target className="w-4 h-4" />
            </div>
            <h2 className="text-base font-bold font-mono text-white tracking-tight">
              Prediction Markets Synthetic Arrow-Debreu Parity
            </h2>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            Risk-free synthetic arbitrage between Polymarket CTF CLOB and Kalshi CFTC-regulated binary contracts.
          </p>
        </div>

        {/* Filter Search */}
        <form onSubmit={handleSearch} className="flex items-center space-x-2">
          <div className="relative">
            <input
              type="text"
              placeholder="Search contract (e.g. FED, BTC)..."
              value={searchTerm}
              onChange={(e) => setSearchTerm(e.target.value)}
              className="w-64 pl-8 pr-3 py-1.5 bg-black/40 border border-white/[0.1] rounded-xl text-white placeholder:text-white/30 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-cyan-400 focus:border-cyan-400"
            />
            <Filter className="w-3.5 h-3.5 text-white/40 absolute left-2.5 top-2.5" />
          </div>
          <button
            type="submit"
            className="px-3.5 py-1.5 bg-cyan-500/20 hover:bg-cyan-500/30 text-cyan-300 border border-cyan-500/40 font-medium text-xs font-mono rounded-xl transition shadow-[0_0_10px_rgba(34,211,238,0.2)]"
          >
            Filter
          </button>
        </form>
      </div>

      {/* Synthetic Formula Explanation Card */}
      <div className="glass-panel border border-white/[0.06] rounded-2xl p-4 flex flex-col md:flex-row items-center justify-between gap-4 font-mono text-xs shadow-2xs">
        <div>
          <div className="font-semibold text-white">Arrow-Debreu Risk-Free Equilibrium Condition</div>
          <div className="text-[11px] text-white/50 mt-0.5">
            Condition: Cost = Price(YES) + Price(NO) + Fees &lt; $1.00 Payout
          </div>
        </div>

        <div className="flex items-center space-x-3 text-[11px]">
          <span className="text-white/50">Polymarket: 0.00% fee</span>
          <span className="text-white/20">/</span>
          <span className="text-white/50">Kalshi: 1.00% fee</span>
          <span className="text-white/20">/</span>
          <span className="text-emerald-400 font-semibold">Payout: $1.00 Guaranteed</span>
        </div>
      </div>

      {/* Opportunities List */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {opportunities.map((opp) => (
          <div
            key={opp.id}
            className="glass-panel border border-white/[0.08] hover:border-cyan-500/40 rounded-2xl p-5 flex flex-col justify-between space-y-4 transition duration-200 shadow-xs"
          >
            <div>
              <div className="flex items-center justify-between mb-2">
                <span className="chip chip-info text-[10px] font-mono font-semibold">
                  {opp.market_title}
                </span>
                <span className="font-mono text-xs font-bold text-emerald-400 tabular-nums">
                  +{opp.roi_pct.toFixed(2)}% Guaranteed ROI
                </span>
              </div>

              {/* Legs Table */}
              <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3.5 space-y-2.5 my-3 font-mono text-xs tabular-nums">
                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-2">
                    <span className="chip chip-pos text-[10px]">
                      BUY YES
                    </span>
                    <span className="text-white font-medium">{opp.buy_yes_venue}</span>
                  </div>
                  <span className="font-semibold text-white">${(opp.yes_price_cents / 100).toFixed(2)}</span>
                </div>

                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-2">
                    <span className="chip chip-neg text-[10px]">
                      BUY NO
                    </span>
                    <span className="text-white font-medium">{opp.buy_no_venue}</span>
                  </div>
                  <span className="font-semibold text-white">${(opp.no_price_cents / 100).toFixed(2)}</span>
                </div>

                <div className="pt-2 border-t border-white/[0.06] flex items-center justify-between text-[11px]">
                  <span className="text-white/50">Total Combined Cost:</span>
                  <span className="font-semibold text-white">${(opp.total_cost_cents / 100).toFixed(2)}</span>
                </div>
              </div>

              {/* Visual Probability Bar */}
              <div className="space-y-1.5">
                <div className="flex justify-between text-[10px] font-mono text-white/50 tabular-nums">
                  <span>YES: {opp.yes_price_cents.toFixed(0)}%</span>
                  <span className="text-emerald-400 font-semibold">Spread: +${(opp.net_profit_cents / 100).toFixed(2)}</span>
                  <span>NO: {opp.no_price_cents.toFixed(0)}%</span>
                </div>
                <div className="h-2 w-full bg-white/[0.08] rounded-full overflow-hidden flex">
                  <div
                    className="bg-emerald-400 h-full"
                    style={{ width: `${opp.yes_price_cents}%` }}
                  />
                  <div
                    className="bg-cyan-400 h-full"
                    style={{ width: `${opp.net_profit_cents}%` }}
                  />
                  <div
                    className="bg-rose-400 h-full"
                    style={{ width: `${opp.no_price_cents}%` }}
                  />
                </div>
              </div>
            </div>

            {/* Bottom Outcome Card & Action */}
            <div className="pt-3 border-t border-white/[0.06] flex items-center justify-between font-mono text-xs">
              <div className="tabular-nums">
                <span className="text-white/50">Profit / 100 contracts: </span>
                <span className="text-emerald-400 font-bold text-sm">
                  +${(opp.net_profit_cents).toFixed(2)} USD
                </span>
              </div>

              {lastExecuted === opp.id ? (
                <span className="chip chip-pos text-xs font-semibold">
                  Executed
                </span>
              ) : (
                <button
                  onClick={() => handleExecute(opp)}
                  disabled={executingId === opp.id}
                  className="px-4 py-2 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white font-medium rounded-xl text-xs font-mono transition shadow-[0_0_14px_rgba(34,211,238,0.25)] flex items-center"
                >
                  <span>{executingId === opp.id ? "Routing..." : "Execute Parity"}</span>
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};
