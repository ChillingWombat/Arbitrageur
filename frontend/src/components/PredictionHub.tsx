import React, { useState, useEffect, useRef } from "react";
import { Filter, ShieldAlert } from "lucide-react";
import type { PredictionOpportunityDto } from "../types";
import { SwitchToggle } from "./SwitchToggle";

interface PredictionHubProps {
  opportunities: PredictionOpportunityDto[];
  onSearch: (market: string) => void;
  onExecutePrediction?: (opp: PredictionOpportunityDto) => Promise<void>;
  loading?: boolean;
  isPaused?: boolean;
}

export const PredictionHub: React.FC<PredictionHubProps> = ({
  opportunities,
  onSearch,
  onExecutePrediction,
  isPaused = false,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const [autoPredictionEnabled, setAutoPredictionEnabled] = useState(false);
  const lastAutoPredExecRef = useRef<number>(0);

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

  // Automated Execution Loop when user manually enables Auto Parity
  useEffect(() => {
    if (!autoPredictionEnabled || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoPredExecRef.current < 6000) return; // 6s cooldown

    const bestOpp = opportunities.find((o) => o.net_profit_cents >= 1.0);
    if (bestOpp) {
      lastAutoPredExecRef.current = now;
      handleExecute(bestOpp);
    }
  }, [autoPredictionEnabled, executingId, opportunities, isPaused]);

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full">
      {/* Header Info */}
      <div className="bg-surface p-5 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-4 shadow-xs">
        <div>
          <h2 className="text-sm font-bold font-mono text-slate-900">
            Prediction Markets Synthetic Arrow-Debreu Parity
          </h2>
          <p className="text-xs text-slate-500 mt-1">
            Risk-free synthetic arbitrage between Polymarket CTF CLOB and Kalshi CFTC-regulated binary contracts.
          </p>
        </div>

        <div className="flex items-center space-x-3">
          {/* Auto Parity Bot Switch (Green for ON, White for OFF) */}
          <div className="flex items-center space-x-1.5 px-2.5 py-1 bg-surface-raised border border-surface-border rounded-lg shadow-2xs">
            <span className="text-xs font-mono font-medium text-slate-700">Auto Bot</span>
            <SwitchToggle
              checked={autoPredictionEnabled}
              onChange={setAutoPredictionEnabled}
              size="sm"
              showStatusText={true}
            />
          </div>

          {/* Filter Search */}
          <form onSubmit={handleSearch} className="flex items-center space-x-2">
            <div className="relative">
              <input
                type="text"
                placeholder="Search contract (e.g. FED, BTC)..."
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                className="w-56 pl-8 pr-3 py-1.5 bg-surface border border-surface-border rounded-lg text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
              <Filter className="w-3.5 h-3.5 text-slate-400 absolute left-2.5 top-2.5" />
            </div>
            <button
              type="submit"
              className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white font-medium text-xs font-mono rounded-lg transition shadow-xs"
            >
              Filter
            </button>
          </form>
        </div>
      </div>

      {/* Synthetic Formula Explanation Card */}
      <div className="bg-surface-raised border border-surface-border rounded-xl p-4 flex flex-col md:flex-row items-center justify-between gap-4 font-mono text-xs shadow-2xs">
        <div>
          <div className="font-semibold text-slate-900">Arrow-Debreu Risk-Free Equilibrium Condition</div>
          <div className="text-[11px] text-slate-500">
            Condition: Cost = Price(YES) + Price(NO) + Fees &lt; $1.00 Payout
          </div>
        </div>

        <div className="flex items-center space-x-3 text-[11px]">
          <span className="text-slate-500">Polymarket: 0.00% fee</span>
          <span className="text-slate-300">/</span>
          <span className="text-slate-500">Kalshi: 1.00% fee</span>
          <span className="text-slate-300">/</span>
          <span className="text-profit font-semibold">Payout: $1.00 Guaranteed</span>
        </div>
      </div>

      {/* Opportunities List */}
      {opportunities.length === 0 ? (
        <div className="bg-surface rounded-2xl border border-surface-border p-12 text-center flex flex-col items-center justify-center space-y-3 shadow-xs">
          <ShieldAlert className="w-9 h-9 text-slate-300" />
          <p className="font-mono text-sm font-bold text-slate-800">No Prediction Market Arbitrage Detected</p>
          <p className="text-xs text-slate-500 max-w-md font-sans leading-relaxed">
            Requires at least 2 enabled & connected prediction venues (e.g. Polymarket and Kalshi) configured with API credentials in System Settings. Zero synthetic data is fabricated.
          </p>
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {opportunities.map((opp) => (
            <div
              key={opp.id}
              className="bg-surface border border-surface-border hover:border-slate-300 rounded-xl p-5 flex flex-col justify-between space-y-4 transition shadow-xs"
            >
              <div>
                <div className="flex items-center justify-between mb-2">
                  <span className="px-2.5 py-0.5 rounded font-mono text-[10px] font-semibold bg-accent-dim text-accent border border-accent-line">
                    {opp.market_title}
                  </span>
                  <span className="font-mono text-xs font-bold text-profit">
                    +{opp.roi_pct.toFixed(2)}% Guaranteed ROI
                  </span>
                </div>

                {/* Legs Table */}
                <div className="bg-surface-raised border border-surface-border rounded-lg p-3 space-y-2.5 my-3 font-mono text-xs">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded bg-profit-dim text-profit border border-profit-border font-semibold text-[10px]">
                        BUY YES
                      </span>
                      <span className="text-slate-900 font-medium">{opp.buy_yes_venue}</span>
                    </div>
                    <span className="font-semibold text-slate-700">${(opp.yes_price_cents / 100).toFixed(2)}</span>
                  </div>

                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded bg-loss-dim text-loss border border-loss-border font-semibold text-[10px]">
                        BUY NO
                      </span>
                      <span className="text-slate-900 font-medium">{opp.buy_no_venue}</span>
                    </div>
                    <span className="font-semibold text-slate-700">${(opp.no_price_cents / 100).toFixed(2)}</span>
                  </div>

                  <div className="pt-2 border-t border-surface-border flex items-center justify-between text-[11px]">
                    <span className="text-slate-500">Total Combined Cost:</span>
                    <span className="font-semibold text-slate-900">${(opp.total_cost_cents / 100).toFixed(2)}</span>
                  </div>
                </div>

                {/* Visual Probability Bar */}
                <div className="space-y-1">
                  <div className="flex justify-between text-[10px] font-mono text-slate-500">
                    <span>YES: {opp.yes_price_cents.toFixed(0)}%</span>
                    <span className="text-profit font-semibold">Spread: +${(opp.net_profit_cents / 100).toFixed(2)}</span>
                    <span>NO: {opp.no_price_cents.toFixed(0)}%</span>
                  </div>
                  <div className="h-2 w-full bg-slate-200 rounded-full overflow-hidden flex">
                    <div
                      className="bg-profit h-full"
                      style={{ width: `${opp.yes_price_cents}%` }}
                    />
                    <div
                      className="bg-accent h-full"
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
                  <span className="text-slate-500">Profit / 100 contracts: </span>
                  <span className="text-profit font-bold text-sm">
                    +${(opp.net_profit_cents).toFixed(2)} USD
                  </span>
                </div>

                {lastExecuted === opp.id ? (
                  <span className="inline-flex items-center px-2 py-0.5 rounded bg-profit-dim text-profit text-xs font-semibold">
                    Executed
                  </span>
                ) : (
                  <button
                    onClick={() => handleExecute(opp)}
                    disabled={executingId === opp.id}
                    className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white font-medium rounded-lg text-xs font-mono transition flex items-center shadow-xs"
                  >
                    <span>{executingId === opp.id ? "Routing..." : "Execute Parity"}</span>
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};
