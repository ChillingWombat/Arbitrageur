import React, { useState, useEffect, useMemo } from "react";
import {
  SlidersHorizontal,
  X,
  Check,
  RotateCcw,
  Info,
  ShieldCheck,
  TrendingUp,
  Coins,
  Layers,
  Sparkles,
} from "lucide-react";
import type { TickerStrategyConfigDto, ArbitrageExecutionMode } from "../types";
import {
  PERP_TICKERS,
  formatCurrencyPrice,
  type PerpTicker,
} from "../types/tickers";
import { getMinCompatibleSize, computeSafeLeverage } from "../services/contractSpecs";
import { api } from "../services/api";
import { InfoTooltip } from "./InfoTooltip";
import { SwitchToggle } from "./SwitchToggle";

export interface TickerConfigModalProps {
  isOpen: boolean;
  onClose: () => void;
  tickerSymbol: string;
  initialStrategy?: "futures" | "funding" | "spot" | string;
  strategyConfig?: TickerStrategyConfigDto;
  isEnabled?: boolean;
  onToggleEnabled?: (enabled: boolean) => void;
  onSaveConfig?: (newConfig: TickerStrategyConfigDto) => Promise<void> | void;
}

const PERCENTILE_OPTIONS = [
  { value: 50, label: "50% (Median - Minimum)" },
  { value: 60, label: "60%" },
  { value: 70, label: "70%" },
  { value: 75, label: "75%" },
  { value: 80, label: "80%" },
  { value: 85, label: "85%" },
  { value: 90, label: "90%" },
  { value: 95, label: "95%" },
  { value: 99, label: "99%" },
];

export const TickerConfigModal: React.FC<TickerConfigModalProps> = ({
  isOpen,
  onClose,
  tickerSymbol,
  initialStrategy = "futures",
  strategyConfig,
  isEnabled,
  onToggleEnabled,
  onSaveConfig,
}) => {
  // Strategy Tab State
  const [activeStrategyTab, setActiveStrategyTab] = useState<"futures" | "funding" | "spot">("futures");

  // Sync active tab with initialStrategy when opening
  useEffect(() => {
    if (initialStrategy === "funding" || initialStrategy === "spot" || initialStrategy === "futures") {
      setActiveStrategyTab(initialStrategy);
    } else {
      setActiveStrategyTab("futures");
    }
  }, [initialStrategy, isOpen]);

  // Find ticker metadata
  const ticker: PerpTicker = useMemo(() => {
    return (
      PERP_TICKERS.find((t) => t.symbol === tickerSymbol) || {
        symbol: tickerSymbol,
        base: tickerSymbol.split(/[-_/]/)[0] || "BTC",
        quote: tickerSymbol.split(/[-_/]/)[1] || "USDC",
        name: tickerSymbol,
        venues: ["HL", "LGT", "VNT", "RH"],
        basePrice: 100,
      }
    );
  }, [tickerSymbol]);

  // Form State for Futures Basis Strategy
  const [targetProfitBps, setTargetProfitBps] = useState<number>(5.0);
  const [windowPercentile, setWindowPercentile] = useState<number>(50);
  const [maxAccumulatedUnits, setMaxAccumulatedUnits] = useState<number>(3);
  const [currentAccumulatedUnits, setCurrentAccumulatedUnits] = useState<number>(0);
  const [executionMode, setExecutionMode] = useState<ArbitrageExecutionMode>("SINGLE_WAY");
  const [unitSize, setUnitSize] = useState<number>(0.005);
  const [userTargetLeverage, setUserTargetLeverage] = useState<number>(5);

  // Form State for Funding Carry Strategy
  const [fundingTradeSizeUsd, setFundingTradeSizeUsd] = useState<number>(25000);
  const [fundingMinAprPct, setFundingMinAprPct] = useState<number>(8.0);
  const [fundingHoldingDays, setFundingHoldingDays] = useState<number>(14);

  // Form State for Spot Flash Strategy
  const [spotTradeSizeUsd, setSpotTradeSizeUsd] = useState<number>(1000);
  const [spotMaxSlippageBps, setSpotMaxSlippageBps] = useState<number>(5.0);
  const [spotMinNetProfitUsd, setSpotMinNetProfitUsd] = useState<number>(2.0);

  // UI status
  const [isSaving, setIsSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);

  // Sync state with strategyConfig or fetch from API
  useEffect(() => {
    if (!isOpen) return;

    if (strategyConfig) {
      setTargetProfitBps(Number(strategyConfig.targetProfitBps ?? 5.0));
      setWindowPercentile(Number(strategyConfig.windowPercentile ?? 50));
      setMaxAccumulatedUnits(Number(strategyConfig.maxAccumulatedUnits ?? 3));
      setCurrentAccumulatedUnits(Number(strategyConfig.currentAccumulatedUnits ?? 0));
      setExecutionMode(strategyConfig.executionMode || "SINGLE_WAY");
      setUnitSize(Number(strategyConfig.unitSize ?? 0.005));
      setUserTargetLeverage(Number(strategyConfig.userTargetLeverage ?? 5));
    } else {
      // Fetch from API
      api
        .getTickerStrategyConfig(tickerSymbol)
        .then((cfg) => {
          if (cfg) {
            setTargetProfitBps(Number(cfg.targetProfitBps ?? 5.0));
            setWindowPercentile(Number(cfg.windowPercentile ?? 50));
            setMaxAccumulatedUnits(Number(cfg.maxAccumulatedUnits ?? 3));
            setCurrentAccumulatedUnits(Number(cfg.currentAccumulatedUnits ?? 0));
            setExecutionMode(cfg.executionMode || "SINGLE_WAY");
            setUnitSize(Number(cfg.unitSize ?? 0.005));
            setUserTargetLeverage(Number(cfg.userTargetLeverage ?? 5));
          }
        })
        .catch((err) => {
          console.warn("Failed to load ticker strategy config in modal:", err);
        });
    }
  }, [isOpen, tickerSymbol, strategyConfig]);

  // Contract specs and minimum compatible lot size
  const minCompatible = useMemo(() => {
    return getMinCompatibleSize(ticker.symbol, ticker.venues, ticker.basePrice);
  }, [ticker]);

  // Compute safe leverage and margin
  const safeLeverage = useMemo(() => {
    return computeSafeLeverage(
      ticker.symbol,
      ticker.venues,
      ticker.basePrice,
      unitSize,
      userTargetLeverage
    );
  }, [ticker, unitSize, userTargetLeverage]);

  // Nominal hurdle calculations (fees + closing friction + target profit)
  const avgFeeBps = 7.0;
  const avgSpreadBps = 0.6;
  const nominalHurdleBps = Math.round((avgFeeBps + avgSpreadBps + targetProfitBps) * 100) / 100;

  // Handle Escape key to close modal
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };
    if (isOpen) {
      window.addEventListener("keydown", handleKeyDown);
    }
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  // Preset Handlers
  const applyPreset = (type: "conservative" | "balanced" | "aggressive") => {
    if (type === "conservative") {
      setTargetProfitBps(8.0);
      setWindowPercentile(80);
      setMaxAccumulatedUnits(2);
      setUserTargetLeverage(5);
      setExecutionMode("DOUBLE_WAY");
    } else if (type === "balanced") {
      setTargetProfitBps(5.0);
      setWindowPercentile(60);
      setMaxAccumulatedUnits(3);
      setUserTargetLeverage(5);
      setExecutionMode("DOUBLE_WAY");
    } else if (type === "aggressive") {
      setTargetProfitBps(2.5);
      setWindowPercentile(50);
      setMaxAccumulatedUnits(5);
      setUserTargetLeverage(15);
      setExecutionMode("SINGLE_WAY");
    }
  };

  const handleResetDefaults = () => {
    setTargetProfitBps(5.0);
    setWindowPercentile(50);
    setMaxAccumulatedUnits(3);
    setCurrentAccumulatedUnits(0);
    setExecutionMode("SINGLE_WAY");
    setUnitSize(minCompatible.minCompatibleSize);
    setUserTargetLeverage(5);
    setFundingTradeSizeUsd(25000);
    setFundingMinAprPct(8.0);
    setFundingHoldingDays(14);
    setSpotTradeSizeUsd(1000);
    setSpotMaxSlippageBps(5.0);
    setSpotMinNetProfitUsd(2.0);
  };

  const handleSave = async () => {
    setIsSaving(true);
    setSaveSuccess(false);

    const updatedConfig: TickerStrategyConfigDto = {
      symbol: ticker.symbol,
      targetProfitBps,
      windowPercentile,
      maxAccumulatedUnits,
      currentAccumulatedUnits,
      executionMode,
      unitSize: Math.max(minCompatible.minCompatibleSize, unitSize),
      userTargetLeverage,
    };

    try {
      await api.saveTickerStrategyConfig(updatedConfig);
      if (onSaveConfig) {
        await onSaveConfig(updatedConfig);
      }
      setSaveSuccess(true);
      setTimeout(() => {
        setSaveSuccess(false);
        onClose();
      }, 400);
    } catch (err) {
      console.error("Failed to save ticker strategy config:", err);
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/40 backdrop-blur-xs flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
      onClick={(e) => {
        if (e.target === e.currentTarget) {
          onClose();
        }
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[960px] h-[680px] max-w-[96vw] max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Modal Top Header Bar */}
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-xl bg-accent-dim border border-accent-line flex items-center justify-center text-accent shadow-xs">
              <SlidersHorizontal className="w-4 h-4" />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h2 className="text-sm font-bold font-mono text-slate-900 leading-tight">
                  {ticker.symbol} Strategy Configurator
                </h2>
                <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-surface-raised text-slate-600 border border-surface-border font-medium">
                  {ticker.name}
                </span>
              </div>
              <p className="text-[11px] text-slate-500 font-mono mt-0.5 flex items-center space-x-2">
                <span>Base Price: {formatCurrencyPrice(ticker.basePrice)}</span>
                <span>•</span>
                <span>Venues: {ticker.venues.join(", ")}</span>
              </p>
            </div>
          </div>

          <div className="flex items-center space-x-3">
            {onToggleEnabled && (
              <div className="flex items-center space-x-2 mr-1">
                <span className="text-xs font-mono text-slate-600 font-medium">Strategy</span>
                <SwitchToggle
                  checked={isEnabled ?? false}
                  onChange={onToggleEnabled}
                  size="sm"
                  showStatusText={true}
                />
              </div>
            )}
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition border border-transparent hover:border-surface-border cursor-pointer"
              title="Close Configurator (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Modal Middle Body: Left Sidebar + Right Content Area */}
        <div className="flex-1 flex min-h-0 overflow-hidden font-mono">
          {/* Left Vertical Strategy Selector Panel */}
          <div className="w-56 shrink-0 border-r border-surface-border bg-slate-50/70 p-3 flex flex-col justify-between select-none overflow-y-auto">
            <div className="space-y-3">
              <div className="px-2 pt-1 flex items-center justify-between">
                <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-slate-400">
                  Strategies
                </span>
                <span className="text-[10px] font-mono text-accent font-semibold">
                  {ticker.symbol}
                </span>
              </div>

              <div className="space-y-1">
                <button
                  type="button"
                  onClick={() => setActiveStrategyTab("futures")}
                  className={`w-full flex items-center space-x-2.5 px-3 py-2.5 rounded-xl font-mono text-xs transition cursor-pointer text-left ${
                    activeStrategyTab === "futures"
                      ? "bg-white text-blue-600 font-bold shadow-xs border border-slate-200/80"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-200/50 border border-transparent"
                  }`}
                >
                  <TrendingUp className={`w-4 h-4 shrink-0 ${activeStrategyTab === "futures" ? "text-blue-600" : "text-slate-400"}`} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate">Futures Convergence</div>
                    <div className="text-[10px] font-normal text-slate-400 truncate">Basis & Dislocation</div>
                  </div>
                </button>

                <button
                  type="button"
                  onClick={() => setActiveStrategyTab("funding")}
                  className={`w-full flex items-center space-x-2.5 px-3 py-2.5 rounded-xl font-mono text-xs transition cursor-pointer text-left ${
                    activeStrategyTab === "funding"
                      ? "bg-white text-blue-600 font-bold shadow-xs border border-slate-200/80"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-200/50 border border-transparent"
                  }`}
                >
                  <Coins className={`w-4 h-4 shrink-0 ${activeStrategyTab === "funding" ? "text-blue-600" : "text-slate-400"}`} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate">Funding Carry</div>
                    <div className="text-[10px] font-normal text-slate-400 truncate">Rate Arbitrage</div>
                  </div>
                </button>

                <button
                  type="button"
                  onClick={() => setActiveStrategyTab("spot")}
                  className={`w-full flex items-center space-x-2.5 px-3 py-2.5 rounded-xl font-mono text-xs transition cursor-pointer text-left ${
                    activeStrategyTab === "spot"
                      ? "bg-white text-blue-600 font-bold shadow-xs border border-slate-200/80"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-200/50 border border-transparent"
                  }`}
                >
                  <Layers className={`w-4 h-4 shrink-0 ${activeStrategyTab === "spot" ? "text-blue-600" : "text-slate-400"}`} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate">Spot Arbitrage</div>
                    <div className="text-[10px] font-normal text-slate-400 truncate">Atomic Flash</div>
                  </div>
                </button>
              </div>
            </div>

            {/* Left Sidebar Bottom Ticker Quick Telemetry Card */}
            <div className="p-2.5 bg-surface border border-surface-border rounded-xl space-y-1.5 font-mono text-[11px]">
              <div className="flex items-center justify-between text-slate-500 text-[10px] uppercase font-bold">
                <span>Asset Telemetry</span>
                <span className="text-accent">{safeLeverage.effectiveLeverage}x Lev</span>
              </div>
              <div className="flex justify-between text-slate-700">
                <span>Min Lot:</span>
                <span className="font-semibold text-slate-900">{minCompatible.minCompatibleSize} {ticker.base}</span>
              </div>
              <div className="flex justify-between text-slate-700">
                <span>Margin/u:</span>
                <span className="font-semibold text-slate-900">${safeLeverage.totalMarginRequiredUsd.toFixed(0)}</span>
              </div>
              <div className="flex justify-between text-slate-700">
                <span>Venues:</span>
                <span className="font-semibold text-slate-900">{ticker.venues.length} Active</span>
              </div>
            </div>
          </div>

          {/* Right Main Content Area */}
          <div className="flex-1 overflow-y-auto p-6 space-y-6 bg-surface">
            {/* TAB 1: Futures Spread Convergence */}
            {activeStrategyTab === "futures" && (
              <div className="space-y-5">
                {/* Presets Bar */}
                <div className="flex flex-wrap items-center justify-between gap-2 p-3 bg-surface-raised rounded-xl border border-surface-border">
                  <div className="flex items-center space-x-1.5 text-xs text-slate-600">
                    <Sparkles className="w-3.5 h-3.5 text-accent" />
                    <span className="font-semibold text-slate-800">Quick Configuration Presets:</span>
                  </div>
                  <div className="flex items-center space-x-1.5 text-xs">
                    <button
                      type="button"
                      onClick={() => applyPreset("conservative")}
                      className="px-2.5 py-1 bg-surface hover:bg-slate-100 text-slate-700 rounded-lg border border-surface-border transition cursor-pointer shadow-2xs"
                      title="Conservative: TP 8 bps, 80% percentile, 2 units, 5x lev"
                    >
                      Conservative
                    </button>
                    <button
                      type="button"
                      onClick={() => applyPreset("balanced")}
                      className="px-2.5 py-1 bg-surface hover:bg-slate-100 text-slate-700 rounded-lg border border-surface-border transition cursor-pointer shadow-2xs"
                      title="Balanced: TP 5 bps, 60% percentile, 3 units, 5x lev"
                    >
                      Balanced
                    </button>
                    <button
                      type="button"
                      onClick={() => applyPreset("aggressive")}
                      className="px-2.5 py-1 bg-surface hover:bg-slate-100 text-slate-700 rounded-lg border border-surface-border transition cursor-pointer shadow-2xs"
                      title="Aggressive: TP 2.5 bps, 50% percentile, 5 units, 15x lev"
                    >
                      Aggressive
                    </button>
                  </div>
                </div>
              {/* Section: Profit Hurdle & Dislocation Filters */}
              <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-3.5 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2">
                  <h3 className="text-xs uppercase font-bold text-slate-900 tracking-wider flex items-center space-x-1.5">
                    <TrendingUp className="w-3.5 h-3.5 text-accent" />
                    <span>Hurdle Rate & Dislocation Triggers</span>
                    <InfoTooltip text="Calculates minimum statistical spread needed for atomic entry." />
                  </h3>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                  {/* Target Profit Input */}
                  <div className="space-y-1.5">
                    <label className="text-xs text-slate-700 font-medium flex items-center justify-between">
                      <span className="flex items-center">
                        Target Profit (BPS)
                        <InfoTooltip text="Net profit cushion locked in per round-trip after deducting taker fees and closing spread." />
                      </span>
                      <span className="text-[10px] text-slate-400">0.1 - 100 bps</span>
                    </label>
                    <div className="flex items-center space-x-2">
                      <input
                        type="number"
                        step="0.5"
                        min="0.1"
                        max="100"
                        value={targetProfitBps}
                        onChange={(e) => setTargetProfitBps(Math.max(0.1, parseFloat(e.target.value) || 0.1))}
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold text-xs focus:outline-none focus:ring-1 focus:ring-accent"
                      />
                      <span className="text-xs text-slate-500 font-semibold">bps</span>
                    </div>
                  </div>

                  {/* Window Percentile Filter */}
                  <div className="space-y-1.5">
                    <label className="text-xs text-slate-700 font-medium flex items-center justify-between">
                      <span className="flex items-center">
                        Historical Window Percentile
                        <InfoTooltip text="Strictly filters entries to spreads at or above this percentile of the rolling sliding window." />
                      </span>
                      <span className="text-[10px] text-slate-400">Min 50%</span>
                    </label>
                    <select
                      value={windowPercentile}
                      onChange={(e) => setWindowPercentile(Math.max(50, parseInt(e.target.value, 10)))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-medium text-xs focus:outline-none focus:ring-1 focus:ring-accent"
                    >
                      {PERCENTILE_OPTIONS.map((opt) => (
                        <option key={opt.value} value={opt.value}>
                          {opt.label}
                        </option>
                      ))}
                    </select>
                  </div>
                </div>

                {/* Nominal Hurdle Visual Breakdown */}
                <div className="bg-blue-50/70 border border-blue-200 rounded-lg p-3 flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 text-xs text-blue-900">
                  <div className="flex items-center space-x-2">
                    <Info className="w-4 h-4 text-blue-600 shrink-0" />
                    <div>
                      <span className="font-bold text-blue-800">Nominal Hurdle:</span>{" "}
                      <strong className="text-blue-700 text-sm">{nominalHurdleBps.toFixed(1)} BPS</strong>
                      <span className="text-[11px] text-slate-500 ml-2">
                        ({avgFeeBps.toFixed(1)} fee + {avgSpreadBps.toFixed(1)} friction + {targetProfitBps.toFixed(1)} target profit)
                      </span>
                    </div>
                  </div>
                  <span className="text-[10px] font-medium bg-white px-2 py-0.5 rounded border border-blue-200 text-blue-800 shrink-0">
                    Gross spread must exceed {nominalHurdleBps.toFixed(1)} BPS
                  </span>
                </div>

                {/* Execution Mode */}
                <div className="pt-2 border-t border-surface-border flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3">
                  <div className="flex items-center">
                    <span className="text-xs font-semibold text-slate-800">Execution Mode</span>
                    <InfoTooltip text="Choose whether to unwind automatically on convergence to zero (Double-Way) or maintain inventory until manual rebalancing (Single-Way)." />
                  </div>
                  <div className="flex items-center space-x-1.5 bg-surface-raised p-1 rounded-lg border border-surface-border text-xs">
                    <button
                      type="button"
                      onClick={() => setExecutionMode("SINGLE_WAY")}
                      className={`px-3 py-1 rounded-md transition cursor-pointer ${
                        executionMode === "SINGLE_WAY"
                          ? "bg-surface text-accent font-semibold shadow-xs border border-surface-border"
                          : "text-slate-600 hover:text-slate-900"
                      }`}
                      title="Single-Way: Enter on wide divergence only"
                    >
                      Single-Way (Divergence)
                    </button>
                    <button
                      type="button"
                      onClick={() => setExecutionMode("DOUBLE_WAY")}
                      className={`px-3 py-1 rounded-md transition cursor-pointer ${
                        executionMode === "DOUBLE_WAY"
                          ? "bg-surface text-accent font-semibold shadow-xs border border-surface-border"
                          : "text-slate-600 hover:text-slate-900"
                      }`}
                      title="Double-Way: Enter on divergence and automatically unwind when spread converges to 0"
                    >
                      Double-Way (Expand & Converge)
                    </button>
                  </div>
                </div>
              </div>

              {/* Section: Sizing, Grid Scaling & Cross-Venue Contract Compatibility */}
              <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-3.5 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2">
                  <h3 className="text-xs uppercase font-bold text-slate-900 tracking-wider flex items-center space-x-1.5">
                    <ShieldCheck className="w-3.5 h-3.5 text-accent" />
                    <span>Position Sizing & Grid Accumulation</span>
                    <InfoTooltip text="Cross-exchange compatible lot sizing, maximum accumulated grid units, and position clearing." />
                  </h3>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs">
                  {/* Unit Size */}
                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium flex items-center justify-between">
                      <span>Unit Size ({ticker.base})</span>
                      <span className="text-[10px] text-slate-400">Per Leg</span>
                    </label>
                    <input
                      type="number"
                      step={minCompatible.stepSize}
                      min={minCompatible.minCompatibleSize}
                      value={unitSize}
                      onChange={(e) =>
                        setUnitSize(Math.max(minCompatible.minCompatibleSize, parseFloat(e.target.value) || minCompatible.minCompatibleSize))
                      }
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <div className="text-[10px] text-slate-500">
                      Notional: ~${(unitSize * ticker.basePrice).toFixed(2)} USD
                    </div>
                  </div>

                  {/* Max Grid Units */}
                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium flex items-center justify-between">
                      <span>Max Grid Units</span>
                      <span className="text-[10px] text-slate-400">1 - 20 Units</span>
                    </label>
                    <input
                      type="number"
                      step="1"
                      min="1"
                      max="20"
                      value={maxAccumulatedUnits}
                      onChange={(e) => setMaxAccumulatedUnits(Math.max(1, Math.min(20, parseInt(e.target.value, 10) || 1)))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <div className="text-[10px] text-slate-500">
                      Max Exposure: ~${(maxAccumulatedUnits * unitSize * ticker.basePrice).toFixed(2)} USD
                    </div>
                  </div>

                  {/* Accumulated Units Reset */}
                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium flex items-center justify-between">
                      <span>Accumulated Units</span>
                      <span className="text-[10px] text-slate-400">Currently Filled</span>
                    </label>
                    <div className="flex items-center space-x-2">
                      <div className="flex-1 px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-800 font-bold text-center">
                        {currentAccumulatedUnits} / {maxAccumulatedUnits}
                      </div>
                      <button
                        type="button"
                        onClick={() => setCurrentAccumulatedUnits(0)}
                        className="px-2.5 py-2 bg-surface hover:bg-slate-100 text-slate-700 border border-surface-border rounded-lg text-[11px] transition cursor-pointer"
                        title="Clear accumulated units back to 0"
                      >
                        <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
                      </button>
                    </div>
                    <div className="text-[10px] text-slate-500">
                      {currentAccumulatedUnits >= maxAccumulatedUnits ? (
                        <span className="text-amber-700 font-semibold">Grid Capacity Full</span>
                      ) : (
                        <span>Available: {maxAccumulatedUnits - currentAccumulatedUnits} units</span>
                      )}
                    </div>
                  </div>
                </div>

                {/* Compatibility Info Bar */}
                <div className="bg-surface-raised p-2.5 rounded-lg border border-surface-border flex items-center justify-between text-[11px] text-slate-600">
                  <div className="flex items-center space-x-2">
                    <span className="font-semibold text-slate-800">Minimum Compatible Lot:</span>
                    <span className="bg-surface px-2 py-0.5 rounded border border-surface-border font-bold text-slate-900">
                      {minCompatible.minCompatibleSize} {ticker.base}
                    </span>
                    <span className="text-slate-500">across ({ticker.venues.join(", ")})</span>
                  </div>
                  <span className="text-slate-400">Step: {minCompatible.stepSize}</span>
                </div>
              </div>

              {/* Section: Safe Futures Leverage & Margin Risk Calculator */}
              <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-3.5 shadow-xs">
                <div className="flex items-center justify-between border-b border-surface-border pb-2">
                  <h3 className="text-xs uppercase font-bold text-slate-900 tracking-wider flex items-center space-x-1.5">
                    <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
                    <span>Safe Effective Leverage & Margin Requirement</span>
                    <InfoTooltip text="Calculates minimum safe leverage ratio and margin requirements across enabled exchanges." />
                  </h3>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-4 gap-3 text-xs">
                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Target Leverage</label>
                    <div className="flex items-center space-x-1.5">
                      <input
                        type="number"
                        step="1"
                        min="1"
                        max="50"
                        value={userTargetLeverage}
                        onChange={(e) =>
                          setUserTargetLeverage(Math.max(1, Math.min(50, parseInt(e.target.value, 10) || 5)))
                        }
                        className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                      />
                      <span className="text-slate-500 font-semibold">x</span>
                    </div>
                  </div>

                  <div className="bg-surface-raised p-2.5 rounded-lg border border-surface-border space-y-0.5">
                    <span className="text-[10px] text-slate-500 uppercase font-semibold">Effective Safe Leverage</span>
                    <div className="text-sm font-bold text-blue-700">{safeLeverage.effectiveLeverage}x</div>
                    <span className="text-[10px] text-slate-400">Max venue: {safeLeverage.maxVenueLeverage}x</span>
                  </div>

                  <div className="bg-surface-raised p-2.5 rounded-lg border border-surface-border space-y-0.5">
                    <span className="text-[10px] text-slate-500 uppercase font-semibold">Margin / Unit (USD)</span>
                    <div className="text-sm font-bold text-slate-900">
                      ${safeLeverage.totalMarginRequiredUsd.toFixed(2)}
                    </div>
                    <span className="text-[10px] text-slate-400">${safeLeverage.marginPerLegUsd.toFixed(2)} / leg</span>
                  </div>

                  <div className="bg-surface-raised p-2.5 rounded-lg border border-surface-border space-y-0.5">
                    <span className="text-[10px] text-slate-500 uppercase font-semibold">Liquidation Buffer</span>
                    <div className="text-sm font-bold text-emerald-700">
                      {safeLeverage.liquidationBufferPct.toFixed(1)}%
                    </div>
                    <span className="text-[10px] text-slate-400">Dual-leg safety margin</span>
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* TAB 2: Funding Carry Radar */}
          {activeStrategyTab === "funding" && (
            <div className="space-y-4">
              <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-4 shadow-xs">
                <div className="border-b border-surface-border pb-2">
                  <h3 className="text-xs uppercase font-bold text-slate-900 tracking-wider flex items-center space-x-1.5">
                    <Coins className="w-3.5 h-3.5 text-accent" />
                    <span>Delta-Neutral Perpetual Funding Carry Settings</span>
                  </h3>
                  <p className="text-[11px] text-slate-500 mt-0.5">
                    Configure position size and annual yield hurdle for {ticker.symbol}.
                  </p>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs">
                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Trade Size (USD)</label>
                    <input
                      type="number"
                      step="1000"
                      min="1000"
                      value={fundingTradeSizeUsd}
                      onChange={(e) => setFundingTradeSizeUsd(Math.max(100, parseFloat(e.target.value) || 1000))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <p className="text-[10px] text-slate-400">Notional per leg</p>
                  </div>

                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Min APR Hurdle (%)</label>
                    <input
                      type="number"
                      step="0.5"
                      min="1"
                      value={fundingMinAprPct}
                      onChange={(e) => setFundingMinAprPct(Math.max(0.1, parseFloat(e.target.value) || 1))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <p className="text-[10px] text-slate-400">Ignore opportunities below this net APR</p>
                  </div>

                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Holding Horizon (Days)</label>
                    <input
                      type="number"
                      step="1"
                      min="1"
                      max="365"
                      value={fundingHoldingDays}
                      onChange={(e) => setFundingHoldingDays(Math.max(1, parseInt(e.target.value, 10) || 14))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <p className="text-[10px] text-slate-400">Projected carry amortization duration</p>
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* TAB 3: Spot Arbitrage */}
          {activeStrategyTab === "spot" && (
            <div className="space-y-4">
              <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-4 shadow-xs">
                <div className="border-b border-surface-border pb-2">
                  <h3 className="text-xs uppercase font-bold text-slate-900 tracking-wider flex items-center space-x-1.5">
                    <Layers className="w-3.5 h-3.5 text-accent" />
                    <span>Cross-Exchange Spot Arbitrage Parameters</span>
                  </h3>
                  <p className="text-[11px] text-slate-500 mt-0.5">
                    Parameters for simultaneous dual-leg spot balance execution for {ticker.symbol}.
                  </p>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-3 gap-4 text-xs">
                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Trade Size (USD)</label>
                    <input
                      type="number"
                      step="100"
                      min="100"
                      value={spotTradeSizeUsd}
                      onChange={(e) => setSpotTradeSizeUsd(Math.max(10, parseFloat(e.target.value) || 500))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <p className="text-[10px] text-slate-400">Capital deployed per flash cycle</p>
                  </div>

                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Max Slippage (BPS)</label>
                    <input
                      type="number"
                      step="0.5"
                      min="0.5"
                      max="50"
                      value={spotMaxSlippageBps}
                      onChange={(e) => setSpotMaxSlippageBps(Math.max(0.1, parseFloat(e.target.value) || 5))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <p className="text-[10px] text-slate-400">Orderbook tolerance limit</p>
                  </div>

                  <div className="space-y-1.5">
                    <label className="text-slate-700 font-medium">Min Realized Net Profit (USD)</label>
                    <input
                      type="number"
                      step="0.5"
                      min="0.1"
                      value={spotMinNetProfitUsd}
                      onChange={(e) => setSpotMinNetProfitUsd(Math.max(0.1, parseFloat(e.target.value) || 1))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <p className="text-[10px] text-slate-400">Hurdle after transfer & taker fees</p>
                  </div>
                </div>
              </div>
            </div>
          )}
          </div>
        </div>

        {/* Modal Bottom Footer Actions */}
        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-between shrink-0 font-mono text-xs">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-lg border border-surface-border transition cursor-pointer"
            title="Reset to default quantitative parameters"
          >
            <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
            <span>Reset Defaults</span>
          </button>

          <div className="flex items-center space-x-3">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-lg font-medium transition cursor-pointer"
            >
              Cancel
            </button>

            <button
              type="button"
              onClick={handleSave}
              disabled={isSaving}
              className={`flex items-center space-x-1.5 px-4 py-1.5 rounded-lg font-semibold transition shadow-xs cursor-pointer ${
                saveSuccess
                  ? "bg-profit text-white border border-profit-border"
                  : "bg-accent hover:bg-blue-600 text-white"
              }`}
            >
              {saveSuccess ? (
                <>
                  <Check className="w-4 h-4" />
                  <span>Saved!</span>
                </>
              ) : (
                <>
                  <SlidersHorizontal className="w-3.5 h-3.5" />
                  <span>{isSaving ? "Saving..." : "Save & Apply"}</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
