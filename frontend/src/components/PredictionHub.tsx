import React, { useState, useEffect, useRef } from "react";
import { Filter, SlidersHorizontal } from "lucide-react";
import type { PredictionOpportunityDto } from "../types";
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
  onOpenConfig?: (symbol: string, strategy?: string) => void;
}

export const PredictionHub: React.FC<PredictionHubProps> = ({
  opportunities,
  onSearch,
  onExecutePrediction,
  isPaused = false,
  hasApiConnectivity = false,
  onOpenConfig,
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
    <div className="p-4 pr-5 space-y-4 overflow-y-auto h-full">
      {/* Strategy Ribbon — venue params left, search + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          <div
            className="ribbon-pill"
            title="Minimum net profit required for auto-execution"
          >
            <span className="text-slate-400">Min Net</span>
            <span className="font-bold text-slate-900 tabular-nums">1.0</span>
            <span className="text-slate-400">¢</span>
          </div>
          <div
            className="ribbon-pill opacity-60"
            title="Min event volume filter (auto_min_event_volume_usd) is defined but disabled: no venue volume data — prediction adapters are scaffolds"
          >
            <span className="text-slate-400">Min Vol</span>
            <span className="font-bold text-slate-500">no venue volume data</span>
          </div>
          <div
            className="ribbon-pill opacity-60"
            title="Max slippage gate (prediction_max_slippage_bps) is defined but disabled: no execution path — prediction adapters are scaffolds"
          >
            <span className="text-slate-400">Max Slip</span>
            <span className="font-bold text-slate-500">no execution path</span>
          </div>
        </div>

        <div className="flex items-center gap-3 shrink-0">
          <button
            type="button"
            onClick={() => onOpenConfig?.("PREDICTION", "prediction")}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Prediction Market strategy parameters"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configurator</span>
          </button>
          {/* Filter Search */}
          <form onSubmit={handleSearch} className="flex items-center space-x-2">
            <div className="relative">
              <input
                type="text"
                placeholder="Search contract (e.g. FED, BTC)..."
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                className="input-field !w-56 !pl-8 !pr-3 !py-1.5 !text-xs font-mono"
              />
              <Filter className="w-3.5 h-3.5 text-slate-400 absolute left-2.5 top-2.5" />
            </div>
            <button
              type="submit"
              className="btn-secondary !py-2 !px-2 !text-xs font-mono cursor-pointer"
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
            <span className="text-xs font-mono text-loss max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
        </div>
      </div>

      {/* Opportunities List */}
      {opportunities.length === 0 ? (
        <div className="bg-surface rounded-xl border border-dashed border-surface-border shadow-card">
          <EmptyState plain title="No Opportunity Detected" />
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {opportunities.map((opp) => (
            <div
              key={opp.id}
              className="bg-surface border border-surface-border hover:border-slate-300 rounded-xl p-4 flex flex-col justify-between space-y-3 transition shadow-card"
            >
              <div>
                <div className="flex items-center justify-between mb-2">
                  <span className="px-2.5 py-0.5 rounded-md font-mono text-xs font-semibold bg-accent-dim text-accent border border-accent-line">
                    {opp.market_title}
                  </span>
                  <span className="font-mono text-xs font-bold text-profit tabular-nums">
                    +{opp.roi_pct.toFixed(2)}% Indicative ROI
                  </span>
                </div>

                {/* Legs Table */}
                <div className="bg-surface-raised border border-surface-border rounded-lg p-3 space-y-2.5 my-3 font-mono text-xs">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded-md bg-profit-dim text-profit border border-profit-border font-semibold text-xs">
                        Buy Yes
                      </span>
                      <span className="text-slate-900 font-medium">{opp.buy_yes_venue}</span>
                    </div>
                    <span className="font-semibold text-slate-700 tabular-nums">${(opp.yes_price_cents / 100).toFixed(2)}</span>
                  </div>

                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded-md bg-loss-dim text-loss border border-loss-border font-semibold text-xs">
                        Buy No
                      </span>
                      <span className="text-slate-900 font-medium">{opp.buy_no_venue}</span>
                    </div>
                    <span className="font-semibold text-slate-700 tabular-nums">${(opp.no_price_cents / 100).toFixed(2)}</span>
                  </div>

                  <div className="pt-2 border-t border-surface-border flex items-center justify-between text-xs">
                    <span className="text-slate-500">Total Combined Cost:</span>
                    <span className="font-semibold text-slate-900 tabular-nums">${(opp.total_cost_cents / 100).toFixed(2)}</span>
                  </div>
                </div>

                {/* Visual Probability Bar */}
                <div className="space-y-1">
                  <div className="flex justify-between text-xs font-mono text-slate-500">
                    <span className="tabular-nums">YES: {opp.yes_price_cents.toFixed(0)}%</span>
                    <span className="text-profit font-semibold tabular-nums">Spread: +${(opp.net_profit_cents / 100).toFixed(2)}</span>
                    <span className="tabular-nums">NO: {opp.no_price_cents.toFixed(0)}%</span>
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
                  <span className="text-profit font-bold text-sm tabular-nums">
                    +${(opp.net_profit_cents).toFixed(2)} USD
                  </span>
                </div>

                {lastExecuted === opp.id ? (
                  <span className="inline-flex items-center px-2 py-0.5 rounded-md bg-profit-dim text-profit text-xs font-semibold">
                    Executed
                  </span>
                ) : (
                  <button
                    onClick={() => handleExecute(opp)}
                    disabled={executingId === opp.id}
                    className="btn-primary !px-2 !py-2 !text-xs font-mono transition flex items-center"
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
