import React from "react";
import type { TickerStrategyConfigDto, ArbitrageExecutionMode } from "../types";
import type { PerpTicker } from "../types/tickers";
import { getMinCompatibleSize, computeSafeLeverage } from "../services/contractSpecs";

interface TickerStrategyBarProps {
  ticker: PerpTicker;
  venues: string[];
  config: TickerStrategyConfigDto;
  onChangeConfig: (newConfig: TickerStrategyConfigDto) => void;
  avgFeeBps?: number;
  avgSpreadBps?: number;
}

const PERCENTILE_OPTIONS = [
  { value: 50, label: "50% (Median - Min)" },
  { value: 60, label: "60%" },
  { value: 70, label: "70%" },
  { value: 75, label: "75%" },
  { value: 80, label: "80%" },
  { value: 85, label: "85%" },
  { value: 90, label: "90%" },
  { value: 95, label: "95%" },
  { value: 99, label: "99%" },
];

export const TickerStrategyBar: React.FC<TickerStrategyBarProps> = ({
  ticker,
  venues,
  config,
  onChangeConfig,
  avgFeeBps = 7.0,
  avgSpreadBps = 0.6,
}) => {
  // Extract minimum compatible size across participating venues
  const minCompatible = getMinCompatibleSize(ticker.symbol, venues, ticker.basePrice);

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

  const handleTargetProfitChange = (val: number) => {
    const clamped = Math.max(0.1, Math.min(100.0, Math.round(val * 10) / 10));
    onChangeConfig({ ...config, targetProfitBps: clamped });
  };

  const handlePercentileChange = (p: number) => {
    // Strictly enforce 50% minimum
    const clamped = Math.max(50, Math.min(99, p));
    onChangeConfig({ ...config, windowPercentile: clamped });
  };

  const handleMaxUnitsChange = (units: number) => {
    const clamped = Math.max(1, Math.min(20, units));
    onChangeConfig({ ...config, maxAccumulatedUnits: clamped });
  };

  const handleModeChange = (mode: ArbitrageExecutionMode) => {
    onChangeConfig({ ...config, executionMode: mode });
  };

  const handleUnitSizeChange = (size: number) => {
    const validSize = Math.max(minCompatible.minCompatibleSize, size);
    onChangeConfig({ ...config, unitSize: validSize });
  };

  const handleLeverageTargetChange = (lev: number) => {
    const clamped = Math.max(1, Math.min(50, lev));
    onChangeConfig({ ...config, userTargetLeverage: clamped });
  };

  return (
    <div className="glass-panel p-3 text-xs font-mono rounded-xl mb-3 space-y-2.5 text-white/90">
      {/* Top Row: Quantitative Parameters and Controls */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        {/* Target Profit and Nominal Hurdle */}
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center space-x-1.5 bg-white/[0.04] px-2.5 py-1 rounded-lg border border-white/[0.08]">
            <span className="text-white/50 font-medium">Target Profit:</span>
            <input
              type="number"
              step="0.5"
              min="0.1"
              max="100"
              value={config.targetProfitBps}
              onChange={(e) => handleTargetProfitChange(parseFloat(e.target.value) || 0.1)}
              className="w-14 px-1.5 py-0.5 bg-white/[0.06] border border-white/[0.12] rounded text-white font-semibold tabular-nums focus:outline-none focus:ring-1 focus:ring-cyan-400"
            />
            <span className="text-white/50">bps</span>
          </div>

          {/* Nominal Hurdle Breakdown Badge */}
          <div
            className="flex items-center space-x-1.5 bg-cyan-500/10 border border-cyan-500/25 px-2.5 py-1 rounded-lg text-cyan-200"
            title="Nominal Hurdle = 2×Comm (roundtrip) + Closing Spread Friction + Target Profit"
          >
            <span className="text-white/50">Nominal Hurdle:</span>
            <span className="font-bold text-cyan-300 tabular-nums">{nominalHurdleBps.toFixed(1)} bps</span>
            <span className="text-[10px] text-white/40 tabular-nums">
              ({avgFeeBps.toFixed(1)} fee + {avgSpreadBps.toFixed(1)} friction + {config.targetProfitBps.toFixed(1)} tp)
            </span>
          </div>
        </div>

        {/* Window Percentile Filter (min 50%) */}
        <div className="flex items-center space-x-2 bg-white/[0.04] px-2.5 py-1 rounded-lg border border-white/[0.08]">
          <span className="text-white/50 font-medium">Window Percentile:</span>
          <select
            value={config.windowPercentile}
            onChange={(e) => handlePercentileChange(parseInt(e.target.value, 10))}
            className="bg-[#0C1019] border border-white/[0.12] rounded px-2 py-0.5 text-xs text-white/95 font-medium focus:outline-none focus:ring-1 focus:ring-cyan-400"
          >
            {PERCENTILE_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value} className="bg-[#0C1019] text-white">
                {opt.label}
              </option>
            ))}
          </select>
        </div>

        {/* Execution Mode: Single-Way vs Double-Way */}
        <div className="flex items-center space-x-1 bg-white/[0.03] p-0.5 rounded-lg border border-white/[0.08] text-xs">
          <button
            type="button"
            onClick={() => handleModeChange("SINGLE_WAY")}
            className={`px-2.5 py-1 rounded-md transition ${
              config.executionMode === "SINGLE_WAY"
                ? "bg-white/[0.12] text-cyan-300 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)] border border-cyan-500/30"
                : "text-white/50 hover:text-white/90"
            }`}
            title="Single-Way: Enter orders only when price divergence increases"
          >
            Single-Way (Divergence)
          </button>
          <button
            type="button"
            onClick={() => handleModeChange("DOUBLE_WAY")}
            className={`px-2.5 py-1 rounded-md transition ${
              config.executionMode === "DOUBLE_WAY"
                ? "bg-white/[0.12] text-cyan-300 font-semibold shadow-[0_0_12px_rgba(34,211,238,0.2)] border border-cyan-500/30"
                : "text-white/50 hover:text-white/90"
            }`}
            title="Double-Way: Enter on divergence and automatically unwind when price difference converges to 0"
          >
            Double-Way (Expand & Converge)
          </button>
        </div>
      </div>

      {/* Bottom Row: Position Sizing, Max Grid Units, and Futures Leverage */}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-2 border-t border-white/[0.06] text-[11px]">
        {/* Unit Sizing with Minimum Compatible Badge */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center space-x-1.5 bg-white/[0.04] px-2 py-1 rounded-lg border border-white/[0.08]">
            <span className="text-white/50">Unit Size:</span>
            <input
              type="number"
              step={minCompatible.stepSize}
              min={minCompatible.minCompatibleSize}
              value={config.unitSize}
              onChange={(e) => handleUnitSizeChange(parseFloat(e.target.value) || minCompatible.minCompatibleSize)}
              className="w-20 px-1.5 py-0.5 bg-white/[0.06] border border-white/[0.12] rounded text-white font-medium tabular-nums focus:outline-none focus:ring-1 focus:ring-cyan-400"
            />
            <span className="text-white/80 font-medium">{ticker.base}</span>
          </div>

          <span
            className="px-2 py-0.5 rounded-md bg-white/[0.04] text-white/60 border border-white/[0.08] tabular-nums"
            title={`Minimum cross-exchange compatible lot size across ${venues.join(", ")}`}
          >
            Min Compatible: {minCompatible.minCompatibleSize} {ticker.base}
          </span>
        </div>

        {/* Max Accumulated Units / Grid Scaling */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center space-x-1.5 bg-white/[0.04] px-2 py-1 rounded-lg border border-white/[0.08]">
            <span className="text-white/50">Max Grid Units:</span>
            <input
              type="number"
              step="1"
              min="1"
              max="20"
              value={config.maxAccumulatedUnits}
              onChange={(e) => handleMaxUnitsChange(parseInt(e.target.value, 10) || 1)}
              className="w-12 px-1.5 py-0.5 bg-white/[0.06] border border-white/[0.12] rounded text-white font-medium tabular-nums focus:outline-none focus:ring-1 focus:ring-cyan-400"
            />
          </div>

          {/* Units Status Indicator */}
          <div className="flex items-center space-x-1.5 px-2.5 py-1 rounded-lg bg-white/[0.04] border border-white/[0.08]">
            <span className="text-white/50">Accumulated:</span>
            <span
              className={`font-semibold tabular-nums ${
                config.currentAccumulatedUnits >= config.maxAccumulatedUnits
                  ? "text-amber-400"
                  : config.currentAccumulatedUnits > 0
                  ? "text-cyan-400"
                  : "text-white/70"
              }`}
            >
              {config.currentAccumulatedUnits} / {config.maxAccumulatedUnits} Units
            </span>
          </div>
        </div>

        {/* Automatic Safe Futures Leverage & Margin Computation */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center space-x-1.5 bg-white/[0.04] px-2 py-1 rounded-lg border border-white/[0.08]">
            <span className="text-white/50">Target Lev:</span>
            <input
              type="number"
              step="1"
              min="1"
              max="50"
              value={config.userTargetLeverage}
              onChange={(e) => handleLeverageTargetChange(parseInt(e.target.value, 10) || 10)}
              className="w-12 px-1.5 py-0.5 bg-white/[0.06] border border-white/[0.12] rounded text-white font-medium tabular-nums focus:outline-none focus:ring-1 focus:ring-cyan-400"
            />
            <span className="text-white/50">x</span>
          </div>

          <div
            className="flex items-center space-x-2 bg-indigo-500/10 border border-indigo-500/25 px-2.5 py-1 rounded-lg text-indigo-200"
            title={`Effective leverage clamped to the most restrictive exchange among active venues (${venues.join(", ")})`}
          >
            <span>
              Safe Lev: <strong className="text-indigo-300 tabular-nums">{safeLeverage.effectiveLeverage}x</strong>
            </span>
            <span className="text-white/20">|</span>
            <span>
              Margin: <strong className="text-white/95 tabular-nums">${safeLeverage.totalMarginRequiredUsd.toFixed(2)}</strong> USD/unit
            </span>
            <span className="text-white/20">|</span>
            <span>
              Buffer: <strong className="text-emerald-400 tabular-nums">{safeLeverage.liquidationBufferPct.toFixed(1)}%</strong>
            </span>
          </div>
        </div>
      </div>
    </div>
  );
};
