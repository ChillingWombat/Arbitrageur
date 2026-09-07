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
    <div className="bg-white border border-slate-200 rounded-lg p-3 text-xs font-mono shadow-xs mb-3 space-y-2.5">
      {/* Top Row: Quantitative Parameters and Controls */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        {/* Target Profit and Nominal Hurdle */}
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center space-x-1.5 bg-slate-50 px-2.5 py-1 rounded border border-slate-200">
            <span className="text-slate-500 font-medium">Target Profit:</span>
            <input
              type="number"
              step="0.5"
              min="0.1"
              max="100"
              value={config.targetProfitBps}
              onChange={(e) => handleTargetProfitChange(parseFloat(e.target.value) || 0.1)}
              className="w-14 px-1.5 py-0.5 bg-white border border-slate-300 rounded text-slate-900 font-semibold focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
            <span className="text-slate-500">bps</span>
          </div>

          {/* Nominal Hurdle Breakdown Badge */}
          <div
            className="flex items-center space-x-1.5 bg-blue-50/70 border border-blue-200 px-2.5 py-1 rounded text-blue-900"
            title="Nominal Hurdle = 2×Comm (roundtrip) + Closing Spread Friction + Target Profit"
          >
            <span className="text-slate-500">Nominal Hurdle:</span>
            <span className="font-bold text-blue-700">{nominalHurdleBps.toFixed(1)} bps</span>
            <span className="text-[10px] text-slate-500">
              ({avgFeeBps.toFixed(1)} fee + {avgSpreadBps.toFixed(1)} friction + {config.targetProfitBps.toFixed(1)} tp)
            </span>
          </div>
        </div>

        {/* Window Percentile Filter (min 50%) */}
        <div className="flex items-center space-x-2 bg-slate-50 px-2.5 py-1 rounded border border-slate-200">
          <span className="text-slate-500 font-medium">Window Percentile:</span>
          <select
            value={config.windowPercentile}
            onChange={(e) => handlePercentileChange(parseInt(e.target.value, 10))}
            className="bg-white border border-slate-300 rounded px-2 py-0.5 text-xs text-slate-900 font-medium focus:outline-none focus:ring-1 focus:ring-blue-500"
          >
            {PERCENTILE_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
        </div>

        {/* Execution Mode: Single-Way vs Double-Way */}
        <div className="flex items-center space-x-1 bg-slate-100 p-0.5 rounded border border-slate-200 text-xs">
          <button
            type="button"
            onClick={() => handleModeChange("SINGLE_WAY")}
            className={`px-2 py-1 rounded transition ${
              config.executionMode === "SINGLE_WAY"
                ? "bg-white text-blue-700 font-semibold shadow-xs border border-slate-200"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Single-Way: Enter orders only when price divergence increases"
          >
            Single-Way (Divergence)
          </button>
          <button
            type="button"
            onClick={() => handleModeChange("DOUBLE_WAY")}
            className={`px-2 py-1 rounded transition ${
              config.executionMode === "DOUBLE_WAY"
                ? "bg-white text-blue-700 font-semibold shadow-xs border border-slate-200"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Double-Way: Enter on divergence and automatically unwind when price difference converges to 0"
          >
            Double-Way (Expand & Converge)
          </button>
        </div>
      </div>

      {/* Bottom Row: Position Sizing, Max Grid Units, and Futures Leverage */}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-1 border-t border-slate-100 text-[11px]">
        {/* Unit Sizing with Minimum Compatible Badge */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center space-x-1.5 bg-slate-50 px-2 py-1 rounded border border-slate-200">
            <span className="text-slate-500">Unit Size:</span>
            <input
              type="number"
              step={minCompatible.stepSize}
              min={minCompatible.minCompatibleSize}
              value={config.unitSize}
              onChange={(e) => handleUnitSizeChange(parseFloat(e.target.value) || minCompatible.minCompatibleSize)}
              className="w-20 px-1.5 py-0.5 bg-white border border-slate-300 rounded text-slate-900 font-medium focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
            <span className="text-slate-600 font-medium">{ticker.base}</span>
          </div>

          <span
            className="px-2 py-0.5 rounded bg-slate-100 text-slate-600 border border-slate-200"
            title={`Minimum cross-exchange compatible lot size across ${venues.join(", ")}`}
          >
            Min Compatible: {minCompatible.minCompatibleSize} {ticker.base}
          </span>
        </div>

        {/* Max Accumulated Units / Grid Scaling */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center space-x-1.5 bg-slate-50 px-2 py-1 rounded border border-slate-200">
            <span className="text-slate-500">Max Grid Units:</span>
            <input
              type="number"
              step="1"
              min="1"
              max="20"
              value={config.maxAccumulatedUnits}
              onChange={(e) => handleMaxUnitsChange(parseInt(e.target.value, 10) || 1)}
              className="w-12 px-1.5 py-0.5 bg-white border border-slate-300 rounded text-slate-900 font-medium focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
          </div>

          {/* Units Status Indicator */}
          <div className="flex items-center space-x-1 px-2 py-1 rounded bg-slate-50 border border-slate-200">
            <span className="text-slate-500">Accumulated:</span>
            <span
              className={`font-semibold ${
                config.currentAccumulatedUnits >= config.maxAccumulatedUnits
                  ? "text-amber-700"
                  : config.currentAccumulatedUnits > 0
                  ? "text-blue-700"
                  : "text-slate-700"
              }`}
            >
              {config.currentAccumulatedUnits} / {config.maxAccumulatedUnits} Units
            </span>
          </div>
        </div>

        {/* Automatic Safe Futures Leverage & Margin Computation */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center space-x-1.5 bg-slate-50 px-2 py-1 rounded border border-slate-200">
            <span className="text-slate-500">Target Lev:</span>
            <input
              type="number"
              step="1"
              min="1"
              max="50"
              value={config.userTargetLeverage}
              onChange={(e) => handleLeverageTargetChange(parseInt(e.target.value, 10) || 10)}
              className="w-12 px-1.5 py-0.5 bg-white border border-slate-300 rounded text-slate-900 font-medium focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
            <span className="text-slate-500">x</span>
          </div>

          <div
            className="flex items-center space-x-2 bg-blue-50/60 border border-blue-200 px-2.5 py-1 rounded text-blue-900"
            title={`Effective leverage clamped to the most restrictive exchange among active venues (${venues.join(", ")})`}
          >
            <span>
              Safe Lev: <strong className="text-blue-700">{safeLeverage.effectiveLeverage}x</strong>
            </span>
            <span className="text-slate-300">|</span>
            <span>
              Margin: <strong className="text-slate-800">${safeLeverage.totalMarginRequiredUsd.toFixed(2)}</strong> USD/unit
            </span>
            <span className="text-slate-300">|</span>
            <span>
              Buffer: <strong className="text-emerald-700">{safeLeverage.liquidationBufferPct.toFixed(1)}%</strong>
            </span>
          </div>
        </div>
      </div>
    </div>
  );
};
