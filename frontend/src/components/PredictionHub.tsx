import React, { useState, useEffect, useRef } from "react";
import { Filter } from "lucide-react";
import type { PredictionOpportunityDto } from "../types";
import { EXCHANGES_ALPHABETICAL } from "./EngineSettings";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { EmptyState } from "./EmptyState";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface PredictionHubProps {
  opportunities: PredictionOpportunityDto[];
  onSearch: (market: string) => void;
  onExecutePrediction?: (opp: PredictionOpportunityDto) => Promise<void>;
  loading?: boolean;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
}

export const PredictionHub: React.FC<PredictionHubProps> = ({
  opportunities,
  onSearch,
  onExecutePrediction,
  isPaused = false,
  hasApiConnectivity = false,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  // Prediction parity positions persist until market resolution — lifecycle
  // is strategy-wide (markets are not perp symbols).
  const bot = useBotLifecycle({ strategyId: "prediction" });
  const lastAutoPredExecRef = useRef<number>(0);

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    onSearch(searchTerm);
  };

  const handleExecute = async (opp: PredictionOpportunityDto) => {
    if (!onExecutePrediction) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") return;
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
    if (!bot.entriesAllowed || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoPredExecRef.current < 6000) return; // 6s cooldown

    const bestOpp = opportunities.find((o) => o.net_profit_cents >= 1.0);
    if (bestOpp) {
      lastAutoPredExecRef.current = now;
      handleExecute(bestOpp);
    }
  }, [bot.entriesAllowed, executingId, opportunities, isPaused]);

  return (
    <div className="p-4 space-y-4 overflow-y-auto h-full">
      {/* Strategy Ribbon — venue params left, search + toggle right */}
      <div className="px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-lg border border-surface-border shadow-2xs">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          {(["polymarket", "kalshi"] as const).map((vid) => {
            const meta = EXCHANGES_ALPHABETICAL.find((e) => e.id === vid);
            return (
              <div
                key={vid}
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title={`${meta?.name ?? vid} taker fee`}
              >
                <span className="text-slate-400">{meta?.name ?? vid} Fee</span>
                <span className="font-bold text-slate-900">{((meta?.feeBps ?? 0) / 100).toFixed(2)}%</span>
              </div>
            );
          })}
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Conditional payout for identical settlement terms"
          >
            <span className="text-slate-400">Payout</span>
            <span className="font-bold text-profit">$1.00</span>
          </div>
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Minimum net profit required for auto-execution"
          >
            <span className="text-slate-400">Min Net</span>
            <span className="font-bold text-slate-900">1.0</span>
            <span className="text-slate-400">¢</span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          {/* Filter Search */}
          <form onSubmit={handleSearch} className="flex items-center space-x-2">
            <div className="relative">
              <input
                type="text"
                placeholder="Search contract (e.g. FED, BTC)..."
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                className="w-56 pl-8 pr-3 py-1.5 bg-surface border border-surface-border rounded-md text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent"
              />
              <Filter className="w-3.5 h-3.5 text-slate-400 absolute left-2.5 top-2.5" />
            </div>
            <button
              type="submit"
              className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white font-medium text-xs font-mono rounded-md transition shadow-2xs"
            >
              Filter
            </button>
          </form>

          {/* Auto Parity Bot Switch (Green for ON, White for OFF) */}
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

      {/* Opportunities List */}
      {opportunities.length === 0 ? (
        <div className="bg-surface rounded-lg border border-dashed border-surface-border shadow-2xs">
          <EmptyState plain title="No Opportunity Detected" />
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {opportunities.map((opp) => (
            <div
              key={opp.id}
              className="bg-surface border border-surface-border hover:border-slate-300 rounded-lg p-4 flex flex-col justify-between space-y-3 transition shadow-2xs"
            >
              <div>
                <div className="flex items-center justify-between mb-2">
                  <span className="px-2.5 py-0.5 rounded-sm font-mono text-xs font-semibold bg-accent-dim text-accent border border-accent-line">
                    {opp.market_title}
                  </span>
                  <span className="font-mono text-xs font-bold text-profit">
                    +{opp.roi_pct.toFixed(2)}% Indicative ROI
                  </span>
                </div>

                {/* Legs Table */}
                <div className="bg-surface-raised border border-surface-border rounded-md p-3 space-y-2.5 my-3 font-mono text-xs">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded-sm bg-profit-dim text-profit border border-profit-border font-semibold text-xs">
                        Buy Yes
                      </span>
                      <span className="text-slate-900 font-medium">{opp.buy_yes_venue}</span>
                    </div>
                    <span className="font-semibold text-slate-700">${(opp.yes_price_cents / 100).toFixed(2)}</span>
                  </div>

                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded-sm bg-loss-dim text-loss border border-loss-border font-semibold text-xs">
                        Buy No
                      </span>
                      <span className="text-slate-900 font-medium">{opp.buy_no_venue}</span>
                    </div>
                    <span className="font-semibold text-slate-700">${(opp.no_price_cents / 100).toFixed(2)}</span>
                  </div>

                  <div className="pt-2 border-t border-surface-border flex items-center justify-between text-xs">
                    <span className="text-slate-500">Total Combined Cost:</span>
                    <span className="font-semibold text-slate-900">${(opp.total_cost_cents / 100).toFixed(2)}</span>
                  </div>
                </div>

                {/* Visual Probability Bar */}
                <div className="space-y-1">
                  <div className="flex justify-between text-xs font-mono text-slate-500">
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
                  <span className="inline-flex items-center px-2 py-0.5 rounded-sm bg-profit-dim text-profit text-xs font-semibold">
                    Executed
                  </span>
                ) : (
                  <button
                    onClick={() => handleExecute(opp)}
                    disabled={executingId === opp.id}
                    className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white font-medium rounded-md text-xs font-mono transition flex items-center shadow-2xs"
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
