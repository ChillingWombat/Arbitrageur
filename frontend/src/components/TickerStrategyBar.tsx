import React, { useState } from "react";
import { SlidersHorizontal, ShieldCheck } from "lucide-react";
import type { TickerStrategyConfigDto } from "../types";
import type { PerpTicker } from "../types/tickers";
import { computeSafeLeverage } from "../services/contractSpecs";
import { TickerConfigModal } from "./TickerConfigModal";

interface TickerStrategyBarProps {
  ticker: PerpTicker;
  venues: string[];
  config: TickerStrategyConfigDto;
  onChangeConfig: (newConfig: TickerStrategyConfigDto) => void;
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
      <div className="bg-surface border border-surface-border rounded-lg p-3 text-xs font-mono shadow-xs mb-3 flex flex-wrap items-center justify-between gap-3 select-none">
        {/* Left: Quantitative Strategy Parameters Summary Chips */}
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center space-x-1.5 text-slate-500 font-medium">
            <span className="w-1.5 h-1.5 rounded-full bg-accent" />
            <span className="font-semibold text-slate-800">Strategy Parameters:</span>
          </div>

          <span
            className="bg-blue-50 text-blue-700 px-2 py-0.5 rounded-sm border border-blue-200/60 font-semibold"
            title="Target profit locked in per round-trip after fees and friction"
          >
            TP {config.targetProfitBps.toFixed(1)} bps
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded-sm border border-surface-border"
            title={`Nominal Hurdle: ${nominalHurdleBps.toFixed(1)} bps (${avgFeeBps.toFixed(1)} fee + ${avgSpreadBps.toFixed(1)} friction + ${config.targetProfitBps.toFixed(1)} tp)`}
          >
            Hurdle <strong className="text-slate-900 font-semibold">{nominalHurdleBps.toFixed(1)} bps</strong>
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded-sm border border-surface-border"
            title="Historical sliding window percentile trigger"
          >
            Window P{config.windowPercentile}%
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded-sm border border-surface-border"
            title="Execution Mode"
          >
            {config.executionMode === "DOUBLE_WAY" ? "Double-Way (Expand & Converge)" : "Single-Way (Divergence)"}
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded-sm border border-surface-border"
            title="Grid unit sizing and current accumulated units"
          >
            Grid <strong className="text-slate-900">{config.currentAccumulatedUnits}/{config.maxAccumulatedUnits}u</strong> ({config.unitSize} {ticker.base})
          </span>

          <span
            className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded-sm border border-surface-border flex items-center space-x-1"
            title={`Effective Safe Leverage: ${safeLeverage.effectiveLeverage}x | Margin: $${safeLeverage.totalMarginRequiredUsd.toFixed(2)} USD/unit | Buffer: ${safeLeverage.liquidationBufferPct.toFixed(1)}%`}
          >
            <ShieldCheck className="w-3 h-3 text-emerald-600" />
            <span>Lev {safeLeverage.effectiveLeverage}x</span>
            <span className="text-slate-300">|</span>
            <span className="text-slate-600">${safeLeverage.totalMarginRequiredUsd.toFixed(0)}/u</span>
          </span>
        </div>

        {/* Right: Configure Strategy Button */}
        <button
          type="button"
          onClick={handleOpenModal}
          className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-md border border-accent-line/60 hover:border-accent transition shadow-xs cursor-pointer"
          title={`Configure ${ticker.symbol} quantitative parameters in pop-out window`}
        >
          <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
          <span>Configure Strategy</span>
        </button>
      </div>

      {/* Internal Modal Fallback */}
      {!onOpenConfigModal && (
        <TickerConfigModal
          isOpen={internalModalOpen}
          onClose={() => setInternalModalOpen(false)}
          tickerSymbol={ticker.symbol}
          strategyConfig={config}
          onSaveConfig={onChangeConfig}
          initialStrategy="futures"
        />
      )}
    </>
  );
};
