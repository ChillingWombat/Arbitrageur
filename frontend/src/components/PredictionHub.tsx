import React, { useState } from "react";
import { Target, CheckCircle, ShieldCheck, Filter, Send, CheckCircle2 } from "lucide-react";
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
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Header Info */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-4">
        <div>
          <h2 className="text-sm font-bold font-mono text-white flex items-center space-x-2">
            <Target className="w-4 h-4 text-accent" />
            <span>Prediction Markets Synthetic Arrow-Debreu Parity</span>
          </h2>
          <p className="text-xs text-slate-400 mt-1">
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
              className="w-64 pl-8 pr-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-white font-mono text-xs focus:outline-none focus:border-accent"
            />
            <Filter className="w-3.5 h-3.5 text-slate-400 absolute left-2.5 top-2.5" />
          </div>
          <button
            type="submit"
            className="px-3 py-1.5 bg-accent hover:bg-cyan-400 text-background font-bold text-xs font-mono rounded-lg transition"
          >
            Filter
          </button>
        </form>
      </div>

      {/* Synthetic Formula Explanation Card */}
      <div className="bg-surface-raised border border-surface-border rounded-xl p-4 flex flex-col md:flex-row items-center justify-between gap-4 font-mono text-xs">
        <div className="flex items-center space-x-3">
          <div className="w-8 h-8 rounded-lg bg-accent/10 border border-accent/30 flex items-center justify-center text-accent">
            <ShieldCheck className="w-5 h-5" />
          </div>
          <div>
            <div className="font-semibold text-white">Arrow-Debreu Risk-Free Equilibrium Condition</div>
            <div className="text-[11px] text-slate-400">
              Condition: Cost = Price(YES) + Price(NO) + Fees &lt; $1.00 Payout
            </div>
          </div>
        </div>

        <div className="flex items-center space-x-4 text-[11px]">
          <span className="text-slate-400">Polymarket: 0.00% fee</span>
          <span className="text-slate-400">•</span>
          <span className="text-slate-400">Kalshi: 1.00% fee</span>
          <span className="text-slate-400">•</span>
          <span className="text-profit font-bold">Payout: $1.00 Guaranteed</span>
        </div>
      </div>

      {/* Opportunities List */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {opportunities.map((opp) => (
          <div
            key={opp.id}
            className="bg-surface border border-surface-border hover:border-accent/40 rounded-xl p-5 flex flex-col justify-between space-y-4 transition"
          >
            <div>
              <div className="flex items-center justify-between mb-2">
                <span className="px-2.5 py-0.5 rounded font-mono text-[10px] font-bold bg-accent/10 text-accent border border-accent/30">
                  {opp.market_title}
                </span>
                <span className="font-mono text-xs font-bold text-profit flex items-center space-x-1">
                  <CheckCircle className="w-3.5 h-3.5" />
                  <span>+{opp.roi_pct.toFixed(2)}% Guaranteed ROI</span>
                </span>
              </div>

              {/* Legs Table */}
              <div className="bg-surface-raised border border-surface-border rounded-lg p-3 space-y-2.5 my-3 font-mono text-xs">
                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-2">
                    <span className="px-1.5 py-0.5 rounded bg-profit/20 text-profit font-bold text-[10px]">
                      BUY YES
                    </span>
                    <span className="text-white font-medium">{opp.buy_yes_venue}</span>
                  </div>
                  <span className="font-bold text-slate-200">{opp.yes_price_cents.toFixed(1)}¢</span>
                </div>

                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-2">
                    <span className="px-1.5 py-0.5 rounded bg-loss/20 text-loss font-bold text-[10px]">
                      BUY NO
                    </span>
                    <span className="text-white font-medium">{opp.buy_no_venue}</span>
                  </div>
                  <span className="font-bold text-slate-200">{opp.no_price_cents.toFixed(1)}¢</span>
                </div>

                <div className="pt-2 border-t border-surface-border flex items-center justify-between text-[11px]">
                  <span className="text-slate-400">Total Combined Cost:</span>
                  <span className="font-semibold text-white">{opp.total_cost_cents.toFixed(1)}¢</span>
                </div>
              </div>

              {/* Visual Probability Bar */}
              <div className="space-y-1">
                <div className="flex justify-between text-[10px] font-mono text-slate-400">
                  <span>YES: {opp.yes_price_cents.toFixed(0)}%</span>
                  <span className="text-profit font-bold">Arb Spread: {opp.net_profit_cents.toFixed(1)}¢</span>
                  <span>NO: {opp.no_price_cents.toFixed(0)}%</span>
                </div>
                <div className="h-2 w-full bg-surface-raised rounded-full overflow-hidden flex">
                  <div
                    className="bg-profit h-full"
                    style={{ width: `${opp.yes_price_cents}%` }}
                  />
                  <div
                    className="bg-accent h-full animate-pulse"
                    style={{ width: `${opp.net_profit_cents}%` }}
                  />
                  <div
                    className="bg-loss h-full"
                    style={{ width: `${opp.no_price_cents}%` }}
                  />
                </div>
              </div>
            </div>

            {/* Bottom Outcome Card & Action */}
            <div className="pt-3 border-t border-surface-border flex items-center justify-between font-mono text-xs">
              <div>
                <span className="text-slate-400">Profit / 100 contracts: </span>
                <span className="text-profit font-bold text-sm">
                  +${(opp.net_profit_cents).toFixed(2)} USD
                </span>
              </div>

              {lastExecuted === opp.id ? (
                <span className="inline-flex items-center space-x-1 text-profit text-xs font-bold">
                  <CheckCircle2 className="w-3.5 h-3.5" />
                  <span>Executed</span>
                </span>
              ) : (
                <button
                  onClick={() => handleExecute(opp)}
                  disabled={executingId === opp.id}
                  className="px-2.5 py-1 bg-accent hover:bg-cyan-400 text-background font-bold rounded-lg text-xs font-mono transition flex items-center space-x-1 glow-cyan"
                >
                  <Send className="w-3 h-3" />
                  <span>{executingId === opp.id ? "Routing..." : "Atomic Parity"}</span>
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};
