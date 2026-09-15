import React, { useState, useEffect, useMemo } from "react";
import {
  SlidersHorizontal,
  X,
  Check,
  RotateCcw,

  ShieldCheck,
  TrendingUp,
  Coins,
  Layers,
} from "lucide-react";
import type { TickerStrategyConfigDto, StrategyParamsDto, ArbitrageExecutionMode, MatrixBaselineSnapshotDto } from "../types";
import {
  PERP_TICKERS,
  formatStrategyName,
  type PerpTicker,
} from "../types/tickers";
import { getMinCompatibleSize, computeSafeLeverage } from "../services/contractSpecs";
import {
  clampWindowValue,
  durationParts,
  durationSeconds,
  formatDuration,
  parseDuration,
  WINDOW_UNIT_BOUNDS,
  type DurationUnit,
} from "../services/matrixQuant";
import { api } from "../services/api";
import { InfoTooltip } from "./InfoTooltip";
import { SwitchToggle } from "./SwitchToggle";

export interface SpotStrategyParams {
  tradeSizeUsd: number;
  minNetSpreadBps: number;
  maxSlippageBps: number;
}

export interface TickerConfigModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** Reference ticker for contract-spec previews (min lot, margin, grid state).
   * The strategy parameters themselves are universal across tickers. */
  tickerSymbol: string;
  initialStrategy?: "futures" | "funding" | "spot" | string;
  onSaveConfig?: (params: StrategyParamsDto) => Promise<void> | void;
}

type SectionIcon = React.ComponentType<{ className?: string }>;

/** Settings sections per strategy — the left panel navigates these, not strategies. */
const STRATEGY_SECTIONS: Record<string, { id: string; label: string; tooltip: string; icon: SectionIcon }[]> = {
  futures: [
    { id: "hurdle", label: "Signal Trigger", tooltip: "Configure the target profit, historical baseline window, percentile trigger, and execution behavior.", icon: TrendingUp },
    { id: "sizing", label: "Sizing & Grid", tooltip: "Configure notional per leg, grid capacity, accumulated units, and cross-venue lot compatibility.", icon: Layers },
    { id: "leverage", label: "Leverage & Margin", tooltip: "Review target leverage, effective safe leverage, margin requirements, and liquidation buffer.", icon: ShieldCheck },
  ],
  funding: [
    { id: "carry", label: "Carry Settings", tooltip: "Configure funding-carry trade size, minimum net APR, and holding horizon.", icon: Coins },
  ],
  spot: [
    { id: "spatial", label: "Spatial Parameters", tooltip: "Configure spot trade size, maximum slippage, and minimum net spread.", icon: Layers },
  ],
};

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
  onSaveConfig,
}) => {
  // Strategy scope is fixed by the page that opened the modal — no switching.
  const [activeStrategyTab, setActiveStrategyTab] = useState<"futures" | "funding" | "spot">("futures");
  const [activeSection, setActiveSection] = useState<string>(
    STRATEGY_SECTIONS.futures[0].id,
  );

  // Sync active tab + reset to the strategy's first section when opening
  useEffect(() => {
    if (!isOpen) return;
    const s =
      initialStrategy === "funding" || initialStrategy === "spot" || initialStrategy === "futures"
        ? initialStrategy
        : "futures";
    setActiveStrategyTab(s);
    setActiveSection((STRATEGY_SECTIONS[s] ?? STRATEGY_SECTIONS.futures)[0].id);
  }, [initialStrategy, isOpen]);

  const sections = STRATEGY_SECTIONS[activeStrategyTab] ?? STRATEGY_SECTIONS.futures;

  // Find ticker metadata
  const ticker: PerpTicker = useMemo(() => {
    return (
      PERP_TICKERS.find((t) => t.symbol === tickerSymbol) || {
        symbol: tickerSymbol || "—",
        base: tickerSymbol.split(/[-_/]/)[0] || "BTC",
        quote: tickerSymbol.split(/[-_/]/)[1] || "USDC",
        name: tickerSymbol,
        venues: ["HL", "LGT", "VNT", "RH"],
        basePrice: 100,
      }
    );
  }, [tickerSymbol]);

  // Form State for Futures Basis Strategy
  const [targetProfitBps, setTargetProfitBps] = useState<number>(1.0);
  const [windowPercentile, setWindowPercentile] = useState<number>(50);
  const [slidingWindowMode, setSlidingWindowMode] = useState<"manual" | "auto">("manual");
  const [windowValue, setWindowValue] = useState<number>(1);
  const [windowUnit, setWindowUnit] = useState<DurationUnit>("days");
  const [baselineSnapshot, setBaselineSnapshot] = useState<MatrixBaselineSnapshotDto | null>(null);
  const [maxAccumulatedUnits, setMaxAccumulatedUnits] = useState<number>(3);
  const [currentAccumulatedUnits, setCurrentAccumulatedUnits] = useState<number>(0);
  const [executionMode, setExecutionMode] = useState<ArbitrageExecutionMode>("SINGLE_WAY");
  const [unitSizeUsd, setUnitSizeUsd] = useState<number>(2500);
  const [userTargetLeverage, setUserTargetLeverage] = useState<number>(5);

  // Form State for Funding Carry Strategy
  const [fundingTradeSizeUsd, setFundingTradeSizeUsd] = useState<number>(25000);
  const [fundingMinAprPct, setFundingMinAprPct] = useState<number>(8.0);
  const [fundingHoldingDays, setFundingHoldingDays] = useState<number>(14);

  // Form State for Spot Flash Strategy
  const [spotTradeSizeUsd, setSpotTradeSizeUsd] = useState<number>(1000);
  const [spotMaxSlippageBps, setSpotMaxSlippageBps] = useState<number>(5.0);
  const [spotMinNetSpreadBps, setSpotMinNetSpreadBps] = useState<number>(15.0);

  // UI status
  const [isSaving, setIsSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);

  // Load universal strategy params + the context ticker's grid state on open
  useEffect(() => {
    if (!isOpen) return;

    api
      .getStrategyParams(activeStrategyTab)
      .then((p) => {
        setTargetProfitBps(Number(p.targetProfitBps ?? 1.0));
        setWindowPercentile(Number(p.windowPercentile ?? 50));
        setSlidingWindowMode(p.slidingWindowMode === "auto" ? "auto" : "manual");
        const secs =
          Number.isFinite(p.slidingWindowSecs) &&
          p.slidingWindowSecs >= 300 &&
          p.slidingWindowSecs <= 2_592_000
            ? p.slidingWindowSecs
            : parseDuration(p.slidingWindow ?? "") ?? 86_400;
        const parts = durationParts(secs);
        setWindowValue(parts.value);
        setWindowUnit(parts.unit);
        setMaxAccumulatedUnits(Number(p.maxAccumulatedUnits ?? 3));
        setExecutionMode((p.executionMode || "SINGLE_WAY") as ArbitrageExecutionMode);
        setUnitSizeUsd(Number(p.unitSizeUsd ?? 2500));
        setUserTargetLeverage(Number(p.userTargetLeverage ?? 5));
        setFundingTradeSizeUsd(Number(p.fundingTradeSizeUsd ?? 25000));
        setFundingMinAprPct(Number(p.fundingMinAprPct ?? 8.0));
        setFundingHoldingDays(Number(p.fundingHoldingDays ?? 14));
        setSpotTradeSizeUsd(Number(p.spotTradeSizeUsd ?? 1000));
        setSpotMaxSlippageBps(Number(p.spotMaxSlippageBps ?? 5.0));
        setSpotMinNetSpreadBps(Number(p.spotMinNetSpreadBps ?? 15.0));
      })
      .catch((err) => {
        console.warn("Failed to load strategy params in modal:", err);
      });

    // Per-ticker runtime grid state (accumulated units) for the context symbol
    api
      .getTickerStrategyConfig(tickerSymbol)
      .then((cfg) => {
        if (cfg) setCurrentAccumulatedUnits(Number(cfg.currentAccumulatedUnits ?? 0));
      })
      .catch((err) => {
        console.warn("Failed to load ticker grid state in modal:", err);
      });
  }, [isOpen, tickerSymbol, activeStrategyTab]);

  const manualWindowSecs = useMemo(() => {
    const secs = durationSeconds(windowValue, windowUnit);
    if (!Number.isFinite(secs)) return 86_400;
    return Math.min(Math.max(Math.round(secs), 300), 2_592_000);
  }, [windowValue, windowUnit]);

  const windowUnitBounds = WINDOW_UNIT_BOUNDS[windowUnit];

  const switchWindowUnit = (u: DurationUnit) => {
    const secs = durationSeconds(windowValue, windowUnit);
    const mult = u === "days" ? 86_400 : u === "hours" ? 3_600 : 60;
    setWindowUnit(u);
    setWindowValue(clampWindowValue(Math.round(secs / mult), u));
  };

  useEffect(() => {
    if (!isOpen || activeStrategyTab !== "futures") {
      setBaselineSnapshot(null);
      return;
    }
    let stopped = false;
    const t = setTimeout(() => {
      api
        .getMatrixBaselines(tickerSymbol, slidingWindowMode, manualWindowSecs, windowPercentile)
        .then((snap) => {
          if (!stopped) setBaselineSnapshot(snap);
        })
        .catch(() => {
          if (!stopped) setBaselineSnapshot(null);
        });
    }, 300);
    return () => {
      stopped = true;
      clearTimeout(t);
    };
  }, [isOpen, activeStrategyTab, tickerSymbol, slidingWindowMode, manualWindowSecs, windowPercentile]);

  const windowStatusLine =
    slidingWindowMode === "manual"
      ? `Manual trailing baseline · ${formatDuration(manualWindowSecs)}`
      : baselineSnapshot && baselineSnapshot.auto_window_secs != null
        ? `Auto-selected ${formatDuration(baselineSnapshot.effective_window_secs)} from ${baselineSnapshot.qualifying_pair_count} qualified venue pairs · median half-life ${formatDuration(Math.round(baselineSnapshot.median_half_life_secs ?? 0))}`
        : `Collecting sufficient history · using manual fallback ${formatDuration(manualWindowSecs)}`;

  // Contract specs and minimum compatible lot size
  const minCompatible = useMemo(() => {
    return getMinCompatibleSize(ticker.symbol, ticker.venues, ticker.basePrice);
  }, [ticker]);

  // Universal sizing is USD notional per leg; derive the context ticker's
  // base-unit quantity (clamped to the venue-compatible minimum lot).
  const unitSize = useMemo(() => {
    if (ticker.basePrice <= 0) return minCompatible.minCompatibleSize;
    const raw = unitSizeUsd / ticker.basePrice;
    const step = minCompatible.stepSize || 0.0001;
    const rounded = Math.ceil(raw / step) * step;
    return Math.max(minCompatible.minCompatibleSize, Math.round(rounded * 1e6) / 1e6);
  }, [unitSizeUsd, ticker.basePrice, minCompatible]);

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

  const handleResetDefaults = () => {
    setTargetProfitBps(1.0);
    setWindowPercentile(50);
    setSlidingWindowMode("manual");
    setWindowValue(1);
    setWindowUnit("days");
    setMaxAccumulatedUnits(3);
    setCurrentAccumulatedUnits(0);
    setExecutionMode("SINGLE_WAY");
    setUnitSizeUsd(2500);
    setUserTargetLeverage(5);
    setFundingTradeSizeUsd(25000);
    setFundingMinAprPct(8.0);
    setFundingHoldingDays(14);
    setSpotTradeSizeUsd(1000);
    setSpotMaxSlippageBps(5.0);
    setSpotMinNetSpreadBps(15.0);
  };

  const handleSave = async () => {
    setIsSaving(true);
    setSaveSuccess(false);

    const canonicalWindow = formatDuration(manualWindowSecs);
    const updatedParams: StrategyParamsDto = {
      strategyId: activeStrategyTab,
      targetProfitBps,
      windowPercentile,
      slidingWindow: canonicalWindow,
      slidingWindowMode,
      slidingWindowSecs: manualWindowSecs,
      maxAccumulatedUnits,
      executionMode,
      unitSizeUsd,
      userTargetLeverage,
      fundingTradeSizeUsd,
      fundingMinAprPct,
      fundingHoldingDays,
      spotTradeSizeUsd,
      spotMaxSlippageBps,
      spotMinNetSpreadBps,
    };

    try {
      await api.saveStrategyParams(updatedParams);
      const tickerConfig: TickerStrategyConfigDto = {
        symbol: ticker.symbol,
        targetProfitBps,
        windowPercentile,
        slidingWindow: canonicalWindow,
        sliding_window: canonicalWindow,
        maxAccumulatedUnits,
        currentAccumulatedUnits,
        executionMode,
        unitSize,
        userTargetLeverage,
      };
      await api.saveTickerStrategyConfig(tickerConfig).catch((err) => {
        console.warn("Failed to persist ticker grid state:", err);
      });
      if (onSaveConfig) {
        await onSaveConfig(updatedParams);
      }
      setSaveSuccess(true);
      setTimeout(() => {
        setSaveSuccess(false);
        onClose();
      }, 400);
    } catch (err) {
      console.error("Failed to save strategy params:", err);
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
      <div className="bg-surface border border-surface-border rounded-xl w-[960px] h-[680px] max-w-[96vw] max-h-[92vh] flex flex-col shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* Modal Top Header Bar */}
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent shadow-2xs">
              <SlidersHorizontal className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 leading-tight">
                {formatStrategyName(activeStrategyTab)} Configurator
              </h2>
            </div>
          </div>

          <div className="flex items-center space-x-3">
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-md text-slate-400 hover:text-slate-700 hover:bg-surface-raised transition border border-transparent hover:border-surface-border cursor-pointer"
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
                <span className="text-xs font-mono font-bold tracking-wider text-slate-400">
                  Settings
                </span>
              </div>

              <div className="space-y-1">
                {sections.map((s) => {
                  const Icon = s.icon;
                  return (
                    <button
                      key={s.id}
                      type="button"
                      onClick={() => setActiveSection(s.id)}
                      className={`w-full flex items-center space-x-2.5 px-3 py-2.5 rounded-lg font-mono text-xs transition cursor-pointer text-left ${
                        activeSection === s.id
                          ? "bg-accent text-white font-bold shadow-2xs border border-accent"
                          : "text-slate-600 hover:text-slate-900 bg-surface hover:bg-slate-100/80 border border-surface-border/60 hover:border-surface-border"
                      }`}
                    >
                      <Icon className={`w-4 h-4 shrink-0 ${activeSection === s.id ? "text-white" : "text-slate-400"}`} />
                      <div className="min-w-0 flex-1 flex items-center">
                        <span className="truncate">{s.label}</span>
                        <InfoTooltip
                          text={s.tooltip}
                          iconClassName={activeSection === s.id ? "text-blue-100 hover:text-white" : undefined}
                        />
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>

          </div>

          {/* Right Main Content Area */}
          <div className="flex-1 overflow-y-auto p-4 space-y-4 bg-surface">
            {/* TAB 1: Futures Spread Convergence */}
            {activeStrategyTab === "futures" && (
              <div className="space-y-5">
              {activeSection === "hurdle" && (
              <div className="space-y-3">
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-xs text-slate-700 font-medium flex items-center justify-between">
                    <span className="flex items-center">
                      Target Profit (BPS)
                      <InfoTooltip text="Net profit cushion locked in per round-trip after deducting taker fees and closing spread." />
                    </span>
                  </label>
                  <div className="flex items-center space-x-2">
                    <input
                      type="number"
                      step="0.5"
                      min="0.1"
                      max="100"
                      value={targetProfitBps}
                      onChange={(e) => setTargetProfitBps(Math.max(0.1, parseFloat(e.target.value) || 0.1))}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
                    />
                    <span className="text-xs text-slate-500 font-semibold">bps</span>
                  </div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-xs text-slate-700 font-medium flex items-center justify-between">
                    <span className="flex items-center">
                      Sliding Window
                      <InfoTooltip text="Trailing duration for the mean-spread baseline and percentile trigger, computed from persisted historical venue spreads. Auto derives the window per ticker from mean-reversion half-life; the manual duration stays as the explicit fallback." />
                    </span>
                    <SwitchToggle
                      size="sm"
                      label="Auto from history"
                      checked={slidingWindowMode === "auto"}
                      onChange={(v) => setSlidingWindowMode(v ? "auto" : "manual")}
                    />
                  </label>
                  <div className="flex items-center space-x-2">
                    <input
                      type="number"
                      step="1"
                      min={windowUnitBounds.min}
                      max={windowUnitBounds.max}
                      value={windowValue}
                      onChange={(e) => {
                        const v = Math.floor(parseFloat(e.target.value));
                        if (Number.isFinite(v)) setWindowValue(clampWindowValue(v, windowUnit));
                      }}
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
                    />
                    <select
                      value={windowUnit}
                      onChange={(e) => switchWindowUnit(e.target.value as DurationUnit)}
                      className="w-32 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-medium text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
                    >
                      <option value="minutes">Minutes</option>
                      <option value="hours">Hours</option>
                      <option value="days">Days</option>
                    </select>
                  </div>
                  <div className="text-xs text-slate-500">{windowStatusLine}</div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-xs text-slate-700 font-medium flex items-center justify-between">
                    <span className="flex items-center">
                      Window Percentile
                      <InfoTooltip text="Strictly filters entries to spreads at or above this percentile of the rolling sliding window." />
                    </span>
                    <span className="text-xs text-slate-400">Min 50%</span>
                  </label>
                  <select
                    value={windowPercentile}
                    onChange={(e) => setWindowPercentile(Math.max(50, parseInt(e.target.value, 10)))}
                    className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-medium text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
                  >
                    {PERCENTILE_OPTIONS.map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                  </select>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-xs text-slate-700 font-medium flex items-center">
                    <span className="flex items-center">
                      Execution Mode
                      <InfoTooltip text="Choose whether to unwind automatically on convergence to zero (Double-Way) or maintain inventory until manual rebalancing (Single-Way)." />
                    </span>
                  </label>
                  <div className="grid grid-cols-2 gap-1.5 bg-surface-raised p-1 rounded-md border border-surface-border text-xs">
                    <button
                      type="button"
                      onClick={() => setExecutionMode("SINGLE_WAY")}
                      className={`w-full px-3 py-1.5 rounded-sm transition cursor-pointer ${
                        executionMode === "SINGLE_WAY"
                          ? "bg-accent text-white shadow-2xs"
                          : "text-slate-500 hover:text-slate-900"
                      }`}
                      title="Single-Way: Enter on wide divergence only"
                    >
                      Single-Way (Divergence)
                    </button>
                    <button
                      type="button"
                      onClick={() => setExecutionMode("DOUBLE_WAY")}
                      className={`w-full px-3 py-1.5 rounded-sm transition cursor-pointer ${
                        executionMode === "DOUBLE_WAY"
                          ? "bg-accent text-white shadow-2xs"
                          : "text-slate-500 hover:text-slate-900"
                      }`}
                      title="Double-Way: Enter on divergence and automatically unwind when spread converges to 0"
                    >
                      Double-Way (Expand & Converge)
                    </button>
                  </div>
                </div>
              </div>

              )}

              {activeSection === "sizing" && (
              <div className="space-y-3 text-xs">
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-slate-700 font-medium flex items-center justify-between">
                    <span>Unit Size (USD)</span>
                    <span className="text-xs text-slate-400">Notional / Leg</span>
                  </label>
                  <input
                    type="number"
                    step={100}
                    min={10}
                    value={unitSizeUsd}
                    onChange={(e) =>
                      setUnitSizeUsd(Math.max(10, parseFloat(e.target.value) || 2500))
                    }
                    className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                  <div className="text-xs text-slate-500">
                    ≈ {unitSize} {ticker.base} on {ticker.symbol}
                  </div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-slate-700 font-medium flex items-center justify-between">
                    <span>Max Grid Units</span>
                    <span className="text-xs text-slate-400">1 - 20 Units</span>
                  </label>
                  <input
                    type="number"
                    step="1"
                    min="1"
                    max="20"
                    value={maxAccumulatedUnits}
                    onChange={(e) => setMaxAccumulatedUnits(Math.max(1, Math.min(20, parseInt(e.target.value, 10) || 1)))}
                    className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                  <div className="text-xs text-slate-500">
                    Max Exposure: ~${(maxAccumulatedUnits * unitSizeUsd).toFixed(0)} USD
                  </div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-slate-700 font-medium flex items-center justify-between">
                    <span>Accumulated Units ({ticker.base})</span>
                    <span className="text-xs text-slate-400">This ticker only</span>
                  </label>
                  <div className="flex items-center space-x-2">
                    <div className="flex-1 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-800 font-bold text-center">
                      {currentAccumulatedUnits} / {maxAccumulatedUnits}
                    </div>
                    <button
                      type="button"
                      onClick={() => setCurrentAccumulatedUnits(0)}
                      className="px-2.5 py-2 bg-surface hover:bg-slate-100 text-slate-700 border border-surface-border rounded-md text-xs transition cursor-pointer"
                      title="Clear accumulated units back to 0"
                    >
                      <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
                    </button>
                  </div>
                  <div className="text-xs text-slate-500">
                    {currentAccumulatedUnits >= maxAccumulatedUnits ? (
                      <span className="text-amber-700 font-semibold">Grid Capacity Full</span>
                    ) : (
                      <span>Available: {maxAccumulatedUnits - currentAccumulatedUnits} units</span>
                    )}
                  </div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between text-xs text-slate-600">
                  <div className="flex items-center space-x-2">
                    <span className="font-semibold text-slate-800">Minimum Compatible Lot ({ticker.base}):</span>
                    <span className="bg-surface-raised px-2 py-0.5 rounded-sm border border-surface-border font-bold text-slate-900">
                      {minCompatible.minCompatibleSize} {ticker.base}
                    </span>
                    <span className="text-slate-500">across ({ticker.venues.join(", ")})</span>
                  </div>
                  <span className="text-slate-400">Step: {minCompatible.stepSize}</span>
                </div>
              </div>

              )}

              {activeSection === "leverage" && (
              <div className="space-y-3 text-xs">
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <label className="text-slate-700 font-medium">Target Leverage</label>
                  <div className="flex items-center space-x-1.5">
                    <input
                      type="number"
                      step="1"
                      min="1"
                      max="5"
                      value={userTargetLeverage}
                      onChange={(e) =>
                        setUserTargetLeverage(Math.max(1, Math.min(5, parseInt(e.target.value, 10) || 5)))
                      }
                      className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <span className="text-slate-500 font-semibold">x</span>
                  </div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <span className="text-xs text-slate-500 font-semibold">Effective Safe Leverage</span>
                  <div className="text-sm font-bold text-blue-700">{safeLeverage.effectiveLeverage}x</div>
                  <span className="text-xs text-slate-400">Max venue: {safeLeverage.maxVenueLeverage}x</span>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <span className="text-xs text-slate-500 font-semibold">Margin / Unit (USD)</span>
                  <div className="text-sm font-bold text-slate-900">
                    ${safeLeverage.totalMarginRequiredUsd.toFixed(2)}
                  </div>
                  <span className="text-xs text-slate-400">${safeLeverage.marginPerLegUsd.toFixed(2)} / leg</span>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                  <span className="text-xs text-slate-500 font-semibold">Liquidation Buffer</span>
                  <div className="text-sm font-bold text-emerald-700">
                    {safeLeverage.liquidationBufferPct.toFixed(1)}%
                  </div>
                  <span className="text-xs text-slate-400">Dual-leg safety margin</span>
                </div>
              </div>
              )}
            </div>
          )}

          {/* TAB 2: Funding Carry Radar */}
          {activeStrategyTab === "funding" && activeSection === "carry" && (
            <div className="space-y-3 text-xs">
              <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                <label className="text-slate-700 font-medium">Trade Size (USD)</label>
                <input
                  type="number"
                  step="1000"
                  min="1000"
                  value={fundingTradeSizeUsd}
                  onChange={(e) => setFundingTradeSizeUsd(Math.max(100, parseFloat(e.target.value) || 1000))}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <p className="text-xs text-slate-400">Notional Per Leg</p>
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                <label className="text-slate-700 font-medium">Min APR Hurdle (%)</label>
                <input
                  type="number"
                  step="0.5"
                  min="1"
                  value={fundingMinAprPct}
                  onChange={(e) => setFundingMinAprPct(Math.max(0.1, parseFloat(e.target.value) || 1))}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <p className="text-xs text-slate-400">Ignore opportunities below this net APR</p>
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                <label className="text-slate-700 font-medium">Holding Horizon (Days)</label>
                <input
                  type="number"
                  step="1"
                  min="1"
                  max="365"
                  value={fundingHoldingDays}
                  onChange={(e) => setFundingHoldingDays(Math.max(1, parseInt(e.target.value, 10) || 14))}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <p className="text-xs text-slate-400">Projected Carry Amortization Duration</p>
              </div>
            </div>
          )}

          {/* TAB 3: Spot Arbitrage */}
          {activeStrategyTab === "spot" && activeSection === "spatial" && (
            <div className="space-y-3 text-xs">
              <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                <label className="text-slate-700 font-medium">Trade Size (USD)</label>
                <input
                  type="number"
                  step="100"
                  min="100"
                  value={spotTradeSizeUsd}
                  onChange={(e) => setSpotTradeSizeUsd(Math.max(10, parseFloat(e.target.value) || 500))}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <p className="text-xs text-slate-400">Capital deployed per flash cycle</p>
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                <label className="text-slate-700 font-medium">Max Slippage (BPS)</label>
                <input
                  type="number"
                  step="0.5"
                  min="0.5"
                  max="50"
                  value={spotMaxSlippageBps}
                  onChange={(e) => setSpotMaxSlippageBps(Math.max(0.1, parseFloat(e.target.value) || 5))}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <p className="text-xs text-slate-400">Orderbook Tolerance Limit</p>
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-1.5 shadow-2xs">
                <label className="text-slate-700 font-medium">Min Net Spread (BPS)</label>
                <input
                  type="number"
                  step="0.5"
                  min="0.1"
                  value={spotMinNetSpreadBps}
                  onChange={(e) => setSpotMinNetSpreadBps(Math.max(0.1, parseFloat(e.target.value) || 1))}
                  className="w-full px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
                <p className="text-xs text-slate-400">Net spread required to auto-execute</p>
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
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-md border border-surface-border transition cursor-pointer"
            title="Reset to default quantitative parameters"
          >
            <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
            <span>Reset Defaults</span>
          </button>

          <div className="flex items-center space-x-3">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-md font-medium transition cursor-pointer"
            >
              Cancel
            </button>

            <button
              type="button"
              onClick={handleSave}
              disabled={isSaving}
              className={`flex items-center space-x-1.5 px-4 py-1.5 rounded-md font-semibold transition shadow-2xs cursor-pointer ${
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
                <span>{isSaving ? "Saving..." : "Save & Apply"}</span>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
