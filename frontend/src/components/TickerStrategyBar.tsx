import React, { useState } from "react";
import { SlidersHorizontal, ShieldCheck } from "lucide-react";
import type { TickerStrategyConfigDto, StrategyParamsDto } from "../types";
import type { PerpTicker } from "../types/tickers";
import { computeSafeLeverage } from "../services/contractSpecs";
import { TickerConfigModal } from "./TickerConfigModal";

interface TickerStrategyBarProps {
  ticker: PerpTicker;
  venues: string[];
  config: TickerStrategyConfigDto;
  onChangeConfig: (params: StrategyParamsDto) => void;
  avgFeeBps?: number;
  avgSpreadBps?: number;
  onOpenConfigModal?: () => void;
}

export const TickerStrategyBar: React.FC<TickerStrategyBarProps> = ({
  ticker,
  venues,
  config,
  onChangeConfig,
  avgFeeBps = 7.0,
  avgSpreadBps = 0.6,
  onOpenConfigModal,
}) => {
  const [internalModalOpen, setInternalModalOpen] = useState(false);

  // Compute safe effective leverage across differing exchange futures contracts
  const safeLeverage = computeSafeLeverage(
    ticker.symbol,
    venues,
    ticker.basePrice,
    config.unitSize,
    config.userTargetLeverage
  );

  // Nominal hurdle = Round-trip Fees + Closing Friction + Target Profit
  const nominalHurdleBps = Math.round((avgFeeBps + avgSpreadBps + config.targetProfitBps) * 100) / 100;

  const handleOpenModal = () => {
    if (onOpenConfigModal) {
      onOpenConfigModal();
    } else {
      setInternalModalOpen(true);
    }
  };

  return (
    <>
      <div className="bg-surface border border-surface-border rounded-xl p-3.5 text-xs font-mono shadow-card mb-3 flex flex-wrap items-center justify-between gap-3 select-none">
        {/* Left: Quantitative Strategy Parameters Summary Chips */}
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center space-x-1.5 text-slate-500 font-medium">
            <span className="w-1.5 h-1.5 rounded-full bg-accent" />
            <span className="font-semibold text-slate-800">Strategy Parameters:</span>
          </div>

          <span
            className="bg-accent-dim text-accent-strong px-2.5 py-1 rounded-lg border border-accent-line/60 font-semibold tabular-nums"
            title="Target profit locked in per round-trip after fees and friction"
          >
            TP {config.targetProfitBps.toFixed(1)} bps
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2.5 py-1 rounded-lg border border-surface-border tabular-nums"
            title={`Nominal Hurdle: ${nominalHurdleBps.toFixed(1)} bps (${avgFeeBps.toFixed(1)} fee + ${avgSpreadBps.toFixed(1)} friction + ${config.targetProfitBps.toFixed(1)} tp)`}
          >
            Hurdle <strong className="text-slate-900 font-semibold">{nominalHurdleBps.toFixed(1)} bps</strong>
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2.5 py-1 rounded-lg border border-surface-border tabular-nums"
            title="Historical sliding window percentile trigger"
          >
            Window P{config.windowPercentile}%
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2.5 py-1 rounded-lg border border-surface-border"
            title="Execution Mode"
          >
            {config.executionMode === "DOUBLE_WAY" ? "Two-Way" : "Single-Way"}
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2.5 py-1 rounded-lg border border-surface-border tabular-nums"
            title="Grid unit sizing and current accumulated units"
          >
            Grid <strong className="text-slate-900">{config.currentAccumulatedUnits}/{config.maxAccumulatedUnits}u</strong> ({config.unitSize} {ticker.base})
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2.5 py-1 rounded-lg border border-surface-border flex items-center space-x-1.5 tabular-nums"
            title={`Effective Safe Leverage: ${safeLeverage.effectiveLeverage}x | Margin: $${safeLeverage.totalMarginRequiredUsd.toFixed(2)} USD/unit | Buffer: ${safeLeverage.liquidationBufferPct.toFixed(1)}%`}
          >
            <ShieldCheck className="w-3.5 h-3.5 text-profit" />
            <span>Lev {safeLeverage.effectiveLeverage}x</span>
            <span className="text-slate-300">|</span>
            <span className="text-slate-600">${safeLeverage.totalMarginRequiredUsd.toFixed(0)}/u</span>
          </span>
        </div>

        {/* Right: Configure Strategy Button */}
        <button
          type="button"
          onClick={handleOpenModal}
          className="btn-secondary flex items-center gap-2 text-xs font-sans"
          title="Configure Simple Spread strategy parameters (applies to all tickers)"
        >
          <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
          <span>Configurator</span>
        </button>
      </div>

      {/* Internal Modal Fallback */}
      {!onOpenConfigModal && (
        <TickerConfigModal
          isOpen={internalModalOpen}
          onClose={() => setInternalModalOpen(false)}
          tickerSymbol={ticker.symbol}
          tickerCatalog={[ticker]}
          onSaveConfig={onChangeConfig}
          initialStrategy="futures"
        />
      )}
    </>
  );
};
