import React, { useState, useEffect, useRef } from "react";
import { Filter, SlidersHorizontal, X } from "lucide-react";
import type { PredictionOpportunityDto, StrategyCapabilityDto } from "../types";
import { BotLifecycleControls, BotPhaseBadge } from "./BotLifecycleControls";
import { EmptyState } from "./EmptyState";
import { ExecuteButton } from "./ExecuteButton";
import { CapacityChart } from "./CapacityChart";
import { DetailField, DetailSection } from "./DetailBits";
import { useBotLifecycle } from "../hooks/useBotLifecycle";
import { api, describeCommandError } from "../services/api";

interface PredictionDetailModalProps {
  opp: PredictionOpportunityDto;
  executing: boolean;
  executeReady: boolean;
  onExecute: (opp: PredictionOpportunityDto) => void;
  onClose: () => void;
}

/** Click-through detail for one parity pair: leg prices, payout math, and
 *  the capacity curve when venue books exist (prediction adapters are
 *  scaffolds — the curve honestly reports "no depth data" until then). */
const PredictionDetailModal: React.FC<PredictionDetailModalProps> = React.memo(
  ({ opp, executing, executeReady, onExecute, onClose }) => {
    useEffect(() => {
      const handleKeyDown = (e: KeyboardEvent) => {
        if (e.key === "Escape") onClose();
      };
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }, [onClose]);
    const cents = (v: number) => `$${(v / 100).toFixed(2)}`;
    return (
      <div
        className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        <div className="bg-surface border border-surface-border rounded-2xl w-[720px] max-w-[95vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="text-sm font-bold font-sans text-slate-900 truncate">{opp.market_title}</span>
              <span className="px-2 py-0.5 rounded-md text-xs font-sans font-semibold border bg-accent-dim text-accent-strong border-accent-line">
                {opp.strategy}
              </span>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-muted hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
              title="Close details (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-5 space-y-5">
            <DetailSection title="Parity Legs">
              <DetailField label="Buy YES @"
                value={`${opp.buy_yes_venue} — ${cents(opp.yes_price_cents)}`}
                tip="Venue and price for the YES leg of the parity."
              />
              <DetailField label="Buy NO @"
                value={`${opp.buy_no_venue} — ${cents(opp.no_price_cents)}`}
                tip="Venue and price for the NO leg of the parity."
              />
              <DetailField label="Combined cost"
                value={cents(opp.total_cost_cents)}
                tip="YES + NO ask cost per hedged contract pair. Below $1.00 is the arbitrage."
              />
            </DetailSection>

            <DetailSection title="Economics">
              <DetailField label="Payout"
                value={cents(opp.conditional_payout_cents)}
                tip="Guaranteed $1.00 settlement per hedged pair when the market resolves."
              />
              <DetailField label="Net profit"
                value={`+${cents(opp.net_profit_cents)} / pair`}
                tip="Payout minus combined cost — locked at resolution regardless of outcome."
              />
              <DetailField label="ROI"
                value={`+${opp.roi_pct.toFixed(2)}%`}
                tip="Net profit over combined cost. Held to market resolution — not a perp convergence."
              />
            </DetailSection>

            <DetailSection
              title="Capacity curve"
              gridClassName="block"
              tip="Profit vs position size walked through the YES/NO orderbooks. Empty until the prediction scan resolves real book depth."
            >
              {(opp.depth_curve?.length ?? 0) >= 2 ? (
                <CapacityChart points={opp.depth_curve ?? []} />
              ) : (
                <span className="text-muted text-xs">
                  no depth data — prediction venue books are not wired yet
                </span>
              )}
            </DetailSection>
          </div>

          <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
            {executeReady ? (
              <ExecuteButton
                executing={executing}
                disabled={!opp.has_arbitrage}
                onClick={() => onExecute(opp)}
                title="execute parity trade"
                className="!text-xs !py-2.5 !px-2.5 cursor-pointer"
              />
            ) : (
              <span className="text-xs font-mono text-muted">execution unavailable</span>
            )}
          </div>
        </div>
      </div>
    );
  },
);


interface PredictionHubProps {
  opportunities: PredictionOpportunityDto[];
  onSearch: (market: string) => void;
  onExecutePrediction?: (opp: PredictionOpportunityDto) => Promise<void>;
  loading?: boolean;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  /** Backend capability descriptor — prediction scan/execute are scaffolds
   *  today, so every armed control is hidden when this says not-ready. */
  capability?: StrategyCapabilityDto;
  minEventVolumeUsd?: number | null;
}

export const PredictionHub: React.FC<PredictionHubProps> = ({
  opportunities,
  onSearch,
  onExecutePrediction,
  isPaused = false,
  hasApiConnectivity = false,
  onOpenConfig,
  capability,
  minEventVolumeUsd,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [detailOpp, setDetailOpp] = useState<PredictionOpportunityDto | null>(null);
  const [executingId, setExecutingId] = useState<string | null>(null);
  const [lastExecuted, setLastExecuted] = useState<string | null>(null);
  const [execError, setExecError] = useState<string | null>(null);
  const execErrorTimer = useRef<number | null>(null);
  // Prediction parity positions persist until market resolution — lifecycle
  // is strategy-wide (markets are not perp symbols).
  const bot = useBotLifecycle({ strategyId: "prediction" });
  const lastAutoPredExecRef = useRef<number>(0);
  const [loadedMinVol, setLoadedMinVol] = useState<number | null>(null);

  useEffect(() => {
    if (minEventVolumeUsd !== undefined && minEventVolumeUsd !== null) return;
    let active = true;
    api
      .getStrategyParams("prediction")
      .then((p) => {
        if (active && p?.autoMinEventVolumeUsd != null) {
          setLoadedMinVol(p.autoMinEventVolumeUsd);
        }
      })
      .catch(() => {
        // Fallback silently if prediction params are unconfigured
      });
    return () => {
      active = false;
    };
  }, [minEventVolumeUsd]);

  const displayMinVol = minEventVolumeUsd ?? loadedMinVol ?? 10_000;

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    onSearch(searchTerm);
  };

  const showExecError = (message: string) => {
    setExecError(message);
    if (execErrorTimer.current != null) window.clearTimeout(execErrorTimer.current);
    execErrorTimer.current = window.setTimeout(() => setExecError(null), 8000);
  };

  // Fail closed: an absent descriptor must not arm a stubbed path.
  const executeReady = capability?.executeReady === true;

  const handleExecute = async (opp: PredictionOpportunityDto, opts?: { silent?: boolean }) => {
    if (!onExecutePrediction || !executeReady) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") return;
    setExecutingId(opp.id);
    try {
      await onExecutePrediction(opp);
      setLastExecuted(opp.id);
      setTimeout(() => setLastExecuted(null), 3000);
    } catch (err: unknown) {
      const message = describeCommandError(err, "Execution failed");
      console.error(`Prediction execution failed for ${opp.id}:`, message);
      if (!opts?.silent) showExecError(message);
    } finally {
      setExecutingId(null);
    }
  };

  // Automated Execution Loop when user manually enables Auto Parity
  useEffect(() => {
    if (!executeReady || !bot.entriesAllowed || executingId || !opportunities.length || isPaused) return;

    const now = Date.now();
    if (now - lastAutoPredExecRef.current < 6000) return; // 6s cooldown

    const bestOpp = opportunities.find((o) => o.net_profit_cents >= 1.0);
    if (bestOpp) {
      lastAutoPredExecRef.current = now;
      void handleExecute(bestOpp, { silent: true });
    }
  }, [executeReady, bot.entriesAllowed, executingId, opportunities, isPaused]);

  return (
    <div className="p-4 pr-5 space-y-4 overflow-y-auto h-full">
      {/* Strategy Ribbon — venue params left, search + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Status
          </span>
          <BotPhaseBadge phase={bot.phase} isPaused={isPaused} />
          <div
            className="ribbon-pill"
            title="Minimum net profit required for auto-execution"
          >
            <span className="text-muted">Min Net</span>
            <span className="font-bold text-slate-900 tabular-nums">1.0</span>
            <span className="text-muted">¢</span>
          </div>
          <div
            className="ribbon-pill"
            title="Minimum event volume threshold for prediction markets"
          >
            <span className="text-muted">Min Vol</span>
            <span className="font-bold text-slate-900 tabular-nums">{displayMinVol.toLocaleString()}</span>
            <span className="text-muted">USD</span>
          </div>
        </div>

        <div className="flex items-center gap-3 shrink-0">
          <button
            type="button"
            onClick={() => onOpenConfig?.("PREDICTION", "prediction")}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Prediction Market strategy parameters"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
            <span>Configurator</span>
          </button>
          {/* Filter Search */}
          <form onSubmit={handleSearch} className="flex items-center space-x-2">
            <div className="relative">
              <input
                type="text"
                placeholder="Search contract..."
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                className="input-field !w-56 !pl-8 !pr-3 !py-1.5 !text-xs font-mono"
              />
              <Filter className="w-3.5 h-3.5 text-muted absolute left-2.5 top-2.5" />
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
            disabled={isPaused || !hasApiConnectivity || !executeReady}
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
          {execError && (
            <span className="text-xs font-mono text-loss max-w-96 truncate" title={execError}>
              Execution blocked: {execError}
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
              className="bg-surface border border-surface-border hover:border-slate-300 rounded-xl p-4 flex flex-col justify-between space-y-3 transition shadow-card cursor-pointer"
              onClick={() => setDetailOpp(opp)}
              title="Click for opportunity details"
            >
              <div>
                <div className="flex items-center justify-between mb-2">
                  <span className="px-2.5 py-0.5 rounded-md font-mono text-xs font-semibold bg-accent-dim text-accent-strong border border-accent-line">
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

                {executeReady && (
                  <span onClick={(e) => e.stopPropagation()}>
                    <ExecuteButton
                      executed={lastExecuted === opp.id}
                      executing={executingId === opp.id}
                      onClick={() => handleExecute(opp)}
                      title="execute parity trade"
                      className="!px-2 !py-2 !text-xs"
                    />
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      {detailOpp && (
        <PredictionDetailModal
          opp={detailOpp}
          executing={executingId === detailOpp.id}
          executeReady={executeReady}
          onExecute={(o) => {
            setDetailOpp(null);
            void handleExecute(o);
          }}
          onClose={() => setDetailOpp(null)}
        />
      )}
    </div>
  );
};
