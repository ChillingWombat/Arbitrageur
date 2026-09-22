import React, { useState, useEffect, useMemo, useCallback } from "react";
import {
  SlidersHorizontal,
  X,
  Check,
  RotateCcw,
  EyeOff,
  ShieldCheck,
  TrendingUp,
  Coins,
  Layers,
  ArrowLeftRight,
  Activity,
  Scale,
  Wind,
  Repeat,
  Target,
} from "lucide-react";
import type {
  TickerStrategyConfigDto,
  StrategyParamsDto,
  PassiveUnwindDto,
  ArbitrageExecutionMode,
  MatrixBaselineSnapshotDto,
  AutotuneRecommendationDto,
  AccountDto,
  StrategyAccountRouteDto,
} from "../types";
import { DEFAULT_PASSIVE_UNWIND } from "../types";
import {
  PERP_TICKERS,
  formatStrategyName,
  type PerpTicker,
} from "../types/tickers";
import { computeSafeLeverage, deriveStepSizing, resolveTotalPaperEquity } from "../services/contractSpecs";
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
import { parseOptThreshold, validateStrategyParams } from "../services/strategyParamsValidate";
import { InfoTooltip } from "./InfoTooltip";
import { SessionProfileCard } from "./SessionProfileCard";
import { SwitchToggle } from "./SwitchToggle";
import { AutotunePreviewModal } from "./AutotunePreviewModal";
import { StrategyRouteCard, STRATEGIES } from "./ExchangeAccounts";

export interface SpotStrategyParams {
  tradeSizeUsd: number;
  minNetSpreadBps: number;
  maxSlippageBps: number;
}

export interface TriangleStrategyParams {
  tradeSizeUsd: number;
  minProfitBps: number;
  maxSlippageBps: number;
}

export interface TickerConfigModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** Reference ticker for contract-spec previews (min lot, margin, grid state).
   * The strategy parameters themselves are universal across tickers. */
  tickerSymbol: string;
  /** Live universe rows; dynamic symbols may not exist in PERP_TICKERS. */
  tickerCatalog?: PerpTicker[];
  initialStrategy?: "futures" | "funding" | "spot" | "triangle" | string;
  onSaveConfig?: (params: StrategyParamsDto) => Promise<void> | void;
}

type SectionIcon = React.ComponentType<{ className?: string }>;

interface SectionDef {
  id: string;
  label: string;
  tooltip: string;
  icon: SectionIcon;
}

/** Settings sections per strategy — the left panel navigates these, not strategies. */
const STRATEGY_SECTIONS: Record<"futures" | "funding" | "spot" | "triangle" | "prediction", SectionDef[]> = {
  futures: [
    { id: "hurdle", label: "Signal Trigger", tooltip: "Configure target profit, historical baseline window, percentile trigger, and execution behavior.", icon: TrendingUp },
    { id: "sizing", label: "Execution & Sizing", tooltip: "Configure notional per leg, grid capacity, accumulated units, and cross-venue lot compatibility.", icon: Layers },
    { id: "leverage", label: "Leverage & Margin", tooltip: "Review target leverage, effective safe leverage, margin requirements, and liquidation buffer.", icon: ShieldCheck },
    { id: "unwind", label: "Passive Unwind", tooltip: "Configure reduce-only limit unwinds pegged inside the spread with timeout and market fallback.", icon: Wind },
    { id: "visibility", label: "Ticker Visibility", tooltip: "Automatic per-strategy hide filters (MIN across venues, fail-closed) plus manual hide list.", icon: EyeOff },
    { id: "routing", label: "Account Routing", tooltip: "Map which exchange accounts to route this strategy's orders to.", icon: ArrowLeftRight },
  ],
  funding: [
    { id: "carry", label: "Carry Parameters", tooltip: "Configure funding-carry trade size and minimum net APR.", icon: Coins },
    { id: "execution", label: "Execution & Slippage", tooltip: "Configure orderbook slippage tolerance, maximum entry spread, and auto-exit threshold.", icon: Activity },
    { id: "risk", label: "Risk & Rebalance", tooltip: "Configure liquidation-distance trim, drift auto-rebalance, and rotation hurdle threshold.", icon: Scale },
    { id: "unwind", label: "Passive Unwind", tooltip: "Configure reduce-only limit unwinds pegged inside the spread with timeout and market fallback.", icon: Wind },
    { id: "visibility", label: "Ticker Visibility", tooltip: "Automatic per-strategy hide filters (MIN across venues, fail-closed) plus manual hide list.", icon: EyeOff },
    { id: "routing", label: "Account Routing", tooltip: "Map which exchange accounts to route this strategy's orders to.", icon: ArrowLeftRight },
  ],
  spot: [
    { id: "spatial", label: "Spatial Parameters", tooltip: "Configure spot trade size, minimum net spread, and orderbook slippage tolerance limit.", icon: Layers },
    { id: "risk", label: "Risk & Safety", tooltip: "Configure liquidation-distance trim protection.", icon: ShieldCheck },
    { id: "unwind", label: "Passive Unwind", tooltip: "Configure reduce-only limit unwinds pegged inside the spread with timeout and market fallback.", icon: Wind },
    { id: "visibility", label: "Ticker Visibility", tooltip: "Automatic per-strategy hide filters (MIN across venues, fail-closed) plus manual hide list.", icon: EyeOff },
    { id: "routing", label: "Account Routing", tooltip: "Map which exchange accounts to route this strategy's orders to.", icon: ArrowLeftRight },
  ],
  triangle: [
    { id: "cycle", label: "Cycle Parameters", tooltip: "Configure synthetic 3-legged arbitrage trade size and minimum net profit hurdle.", icon: Repeat },
    { id: "guardrails", label: "Cycle Guardrails", tooltip: "Configure per-leg slippage tolerance and execution constraints for triangular cycles.", icon: ShieldCheck },
    { id: "visibility", label: "Ticker Visibility", tooltip: "Automatic hide filters (minimum volume/price) plus manual hide list for triangular cycle assets.", icon: EyeOff },
    { id: "routing", label: "Account Routing", tooltip: "Map which exchange accounts to route this strategy's orders to.", icon: ArrowLeftRight },
  ],
  prediction: [
    { id: "parity", label: "Parity Parameters", tooltip: "Minimum net profit threshold and disabled scaffold gates for prediction parity.", icon: Target },
    { id: "visibility", label: "Ticker Visibility", tooltip: "Automatic per-strategy hide filters plus manual hide list.", icon: EyeOff },
    { id: "routing", label: "Account Routing", tooltip: "Map which exchange accounts to route this strategy's orders to.", icon: ArrowLeftRight },
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

/** Standardized row container for config parameters */
interface SettingRowProps {
  label: string;
  description?: string;
  tooltip?: string;
  children: React.ReactNode;
}

const SettingRow: React.FC<SettingRowProps> = ({ label, description, tooltip, children }) => (
  <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-2xs space-y-2 transition hover:border-slate-300">
    <div className="flex items-center gap-3">
      <div className="flex items-center gap-1.5 min-w-0">
        <span className="text-xs font-semibold font-sans text-slate-800">{label}</span>
        {tooltip && <InfoTooltip text={tooltip} />}
      </div>
      <div className="shrink-0 flex items-center gap-2">{children}</div>
    </div>
    {description && (
      <p className="text-[11px] font-sans text-slate-400 font-normal leading-normal border-t border-surface-border/60 pt-2">{description}</p>
    )}
  </div>
);

/** Standardized number input with unit suffix badge */
interface NumberInputProps {
  value: number | string;
  onChange: (val: string) => void;
  unit?: string;
  placeholder?: string;
  step?: string | number;
  min?: string | number;
  max?: string | number;
  disabled?: boolean;
  className?: string;
}

const NumberInput: React.FC<NumberInputProps> = ({
  value,
  onChange,
  unit,
  placeholder,
  step = "any",
  min,
  max,
  disabled = false,
  className = "w-32",
}) => (
  <div className="flex items-center gap-1.5 shrink-0">
    <input
      type="number"
      step={step}
      min={min}
      max={max}
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
      className={`h-9 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono font-bold text-slate-900 tabular-nums focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition disabled:opacity-40 ${className}`}
    />
    {unit && (
      <span className="text-[11px] font-mono font-semibold text-slate-400 select-none min-w-[24px]">
        {unit}
      </span>
    )}
  </div>
);

export const TickerConfigModal: React.FC<TickerConfigModalProps> = ({
  isOpen,
  onClose,
  tickerSymbol,
  tickerCatalog,
  initialStrategy = "futures",
  onSaveConfig,
}) => {
  // Strategy scope is fixed by the page that opened the modal
  const [activeStrategyTab, setActiveStrategyTab] = useState<"futures" | "funding" | "spot" | "triangle" | "prediction">("futures");
  const [activeSection, setActiveSection] = useState<string>(
    STRATEGY_SECTIONS.futures[0].id,
  );

  // Sync active tab + reset to the strategy's first section when opening
  useEffect(() => {
    if (!isOpen) return;
    const s =
      initialStrategy === "funding" ||
      initialStrategy === "spot" ||
      initialStrategy === "futures" ||
      initialStrategy === "triangle" ||
      initialStrategy === "prediction"
        ? (initialStrategy as "futures" | "funding" | "spot" | "triangle" | "prediction")
        : "futures";
    setActiveStrategyTab(s);
    setActiveSection((STRATEGY_SECTIONS[s] ?? STRATEGY_SECTIONS.futures)[0].id);
  }, [initialStrategy, isOpen]);

  const sections = STRATEGY_SECTIONS[activeStrategyTab] ?? STRATEGY_SECTIONS.futures;

  // Find ticker metadata
  const ticker: PerpTicker = useMemo(() => {
    return (
      tickerCatalog?.find((t) => t.symbol === tickerSymbol) ||
      PERP_TICKERS.find((t) => t.symbol === tickerSymbol) || {
        symbol: tickerSymbol || "—",
        base: tickerSymbol.split(/[-_/]/)[0] || "BTC",
        quote: tickerSymbol.split(/[-_/]/)[1] || "USDC",
        name: tickerSymbol,
        venues: ["HL", "LGT", "VNT", "RH"],
        basePrice: 100,
      }
    );
  }, [tickerCatalog, tickerSymbol]);

  // Form State for Futures Basis Strategy
  const [targetProfitBps, setTargetProfitBps] = useState<number>(1.0);
  const [windowPercentile, setWindowPercentile] = useState<number>(50);
  const [slidingWindowMode, setSlidingWindowMode] = useState<"manual" | "auto">("manual");
  const [windowValue, setWindowValue] = useState<number>(1);
  const [windowUnit, setWindowUnit] = useState<DurationUnit>("days");
  const [baselineSnapshot, setBaselineSnapshot] = useState<MatrixBaselineSnapshotDto | null>(null);
  const [autotuneOpen, setAutotuneOpen] = useState(false);
  const [autotuneRec, setAutotuneRec] = useState<AutotuneRecommendationDto | null>(null);
  const [autotuneLoading, setAutotuneLoading] = useState(false);
  const [autotuneError, setAutotuneError] = useState<string | null>(null);
  const [paramsVersion, setParamsVersion] = useState(0);

  const [currentAccumulatedUnits, setCurrentAccumulatedUnits] = useState<number>(0);
  const [executionMode, setExecutionMode] = useState<ArbitrageExecutionMode>("SINGLE_WAY");
  const [stepSizeMult, setStepSizeMult] = useState<number>(10);
  const [maxStepUsd, setMaxStepUsd] = useState<number>(2500);
  const [maxTickerEquityPct, setMaxTickerEquityPct] = useState<number>(20);
  const [totalEquityUsd, setTotalEquityUsd] = useState<number | null>(null);
  const [userTargetLeverage, setUserTargetLeverage] = useState<number>(5);

  // Form State for Funding Carry Strategy
  const [fundingTradeSizeUsd, setFundingTradeSizeUsd] = useState<number>(2000);
  const [fundingMinAprPct, setFundingMinAprPct] = useState<number>(8.0);
  const [fundingHoldingDays, setFundingHoldingDays] = useState<number>(14);
  const [fundingMaxSlippageBps, setFundingMaxSlippageBps] = useState<number>(15.0);
  const [fundingMaxEntrySpreadBps, setFundingMaxEntrySpreadBps] = useState<number>(50.0);
  const [fundingAutoExitSpreadPct, setFundingAutoExitSpreadPct] = useState<string>("");
  const [fundingRotationThresholdBps, setFundingRotationThresholdBps] = useState<string>("");
  const [fundingAutoRebalance, setFundingAutoRebalance] = useState<boolean>(false);
  const [fundingEnableBasis, setFundingEnableBasis] = useState<boolean>(true);
  const [fundingMinBasisBps, setFundingMinBasisBps] = useState<string>("");

  // Form State for Spot Strategy
  const [spotTradeSizeUsd, setSpotTradeSizeUsd] = useState<number>(1000);
  const [spotMaxSlippageBps, setSpotMaxSlippageBps] = useState<number>(5.0);
  const [spotMinNetSpreadBps, setSpotMinNetSpreadBps] = useState<number>(15.0);

  // Form State for Triangle Strategy
  const [triangleTradeSizeUsd, setTriangleTradeSizeUsd] = useState<number>(10000);
  const [triangleMinProfitBps, setTriangleMinProfitBps] = useState<number>(2.0);
  const [triangleMaxSlippageBps, setTriangleMaxSlippageBps] = useState<number>(5.0);

  // Universal trim block (one threshold per strategy, "" = off)
  const [futuresLiqTrimPct, setFuturesLiqTrimPct] = useState<string>("");
  const [fundingLiqTrimPct, setFundingLiqTrimPct] = useState<string>("");
  const [spotLiqTrimPct, setSpotLiqTrimPct] = useState<string>("");

  // Per-strategy slippage for the futures/spread path
  const [futuresMaxSlippageBps, setFuturesMaxSlippageBps] = useState<number>(15.0);

  // Passive limit unwinds/rebalances. Per-strategy mode selects the source:
  // "inherit" → global Engine Settings policy, "custom" → the per-strategy
  // params below, "off" → always market exits.
  const [futuresPassive, setFuturesPassive] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });
  const [fundingPassive, setFundingPassive] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });
  const [spotPassive, setSpotPassive] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });
  const [futuresPassiveMode, setFuturesPassiveMode] = useState<"inherit" | "custom" | "off">("inherit");
  const [fundingPassiveMode, setFundingPassiveMode] = useState<"inherit" | "custom" | "off">("inherit");
  const [spotPassiveMode, setSpotPassiveMode] = useState<"inherit" | "custom" | "off">("inherit");

  // Per-strategy auto-hide filters ("" = off)
  const [autoMinVolumeUsd, setAutoMinVolumeUsd] = useState<string>("");
  const [autoMinPriceUsd, setAutoMinPriceUsd] = useState<string>("");
  const [autoMinEventVolumeUsd, setAutoMinEventVolumeUsd] = useState<number | null>(null);

  // Manual hide list (global, shared by all strategies; applies AFTER auto)
  const [exemptList, setExemptList] = useState<string[] | null>(null);
  const [revealBusy, setRevealBusy] = useState(false);

  // UI status
  const [isSaving, setIsSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Strategy Account Routing state
  const [routes, setRoutes] = useState<StrategyAccountRouteDto[]>([]);
  const [routeAccounts, setRouteAccounts] = useState<AccountDto[]>([]);
  const [routingError, setRoutingError] = useState<string | null>(null);

  const reloadRoutes = useCallback(async () => {
    try {
      const [r, a] = await Promise.all([api.listStrategyRoutes(), api.listAccounts()]);
      setRoutes(r);
      setRouteAccounts(a);
      setRoutingError(null);
    } catch (e) {
      setRoutingError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (isOpen && activeSection === "routing") {
      void reloadRoutes();
    }
  }, [isOpen, activeSection, reloadRoutes]);

  // Load universal strategy params + context ticker grid state on open
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
        setExecutionMode((p.executionMode || "SINGLE_WAY") as ArbitrageExecutionMode);
        setStepSizeMult(Number(p.stepSizeMult ?? 10));
        setMaxStepUsd(Number(p.maxStepUsd ?? 2500));
        setMaxTickerEquityPct(Number(p.maxTickerEquityPct ?? 20));
        setUserTargetLeverage(Number(p.userTargetLeverage ?? 5));
        setFundingTradeSizeUsd(Number(p.fundingTradeSizeUsd ?? 2000));
        setFundingMinAprPct(Number(p.fundingMinAprPct ?? 8.0));
        setFundingHoldingDays(Number(p.fundingHoldingDays ?? 14));
        setFundingMaxSlippageBps(Number(p.fundingMaxSlippageBps ?? 15.0));
        setFundingMaxEntrySpreadBps(Number(p.fundingMaxEntrySpreadBps ?? 50.0));
        setFundingAutoExitSpreadPct(p.fundingAutoExitSpreadPct != null ? String(p.fundingAutoExitSpreadPct) : "");
        setFundingRotationThresholdBps(p.fundingRotationThresholdBps != null ? String(p.fundingRotationThresholdBps) : "");
        setFundingAutoRebalance(p.fundingAutoRebalance === true);
        setFundingEnableBasis((p.fundingEnableBasis ?? true) !== false);
        setFundingMinBasisBps(p.fundingMinBasisBps != null ? String(p.fundingMinBasisBps) : "");
        setFundingLiqTrimPct(p.fundingLiqTrimPct != null ? String(p.fundingLiqTrimPct) : "");
        setFuturesMaxSlippageBps(Number(p.futuresMaxSlippageBps ?? 15.0));
        setFuturesLiqTrimPct(p.futuresLiqTrimPct != null ? String(p.futuresLiqTrimPct) : "");
        setSpotLiqTrimPct(p.spotLiqTrimPct != null ? String(p.spotLiqTrimPct) : "");
        setSpotTradeSizeUsd(Number(p.spotTradeSizeUsd ?? 1000));
        setSpotMaxSlippageBps(Number(p.spotMaxSlippageBps ?? 5.0));
        setSpotMinNetSpreadBps(Number(p.spotMinNetSpreadBps ?? 15.0));
        setTriangleTradeSizeUsd(Number(p.triangleTradeSizeUsd ?? 10000));
        setTriangleMinProfitBps(Number(p.triangleMinProfitBps ?? 2.0));
        setTriangleMaxSlippageBps(Number(p.triangleMaxSlippageBps ?? 5.0));
        setAutoMinVolumeUsd(p.autoMinVolumeUsd != null ? String(p.autoMinVolumeUsd) : "");
        setAutoMinPriceUsd(p.autoMinPriceUsd != null ? String(p.autoMinPriceUsd) : "");
        setAutoMinEventVolumeUsd(p.autoMinEventVolumeUsd ?? null);
        setFuturesPassive({ ...DEFAULT_PASSIVE_UNWIND, ...(p.futuresPassiveUnwind ?? {}) });
        setFundingPassive({ ...DEFAULT_PASSIVE_UNWIND, ...(p.fundingPassiveUnwind ?? {}) });
        setSpotPassive({ ...DEFAULT_PASSIVE_UNWIND, ...(p.spotPassiveUnwind ?? {}) });
        // Legacy rows carry "" — auto-resolve: an enabled per-strategy config
        // was a custom policy before the mode field existed.
        const resolveMode = (m: string | undefined, cfg: PassiveUnwindDto | undefined): "inherit" | "custom" | "off" =>
          m === "inherit" || m === "custom" || m === "off" ? m : (cfg?.enabled ? "custom" : "inherit");
        setFuturesPassiveMode(resolveMode(p.futuresPassiveMode, p.futuresPassiveUnwind));
        setFundingPassiveMode(resolveMode(p.fundingPassiveMode, p.fundingPassiveUnwind));
        setSpotPassiveMode(resolveMode(p.spotPassiveMode, p.spotPassiveUnwind));
      })
      .catch((err) => {
        console.warn("Failed to load strategy params in modal:", err);
      });

    // Manual hide list is global and shared — load it whenever the modal opens
    api
      .getExemptedTickers()
      .then((l) => setExemptList(l))
      .catch(() => {
        setExemptList(null);
      });

    // Per-ticker runtime grid state for the context symbol
    if (activeStrategyTab === "futures") {
      api
        .getTickerStrategyConfig(tickerSymbol)
        .then((cfg) => {
          if (cfg) setCurrentAccumulatedUnits(Number(cfg.currentAccumulatedUnits ?? 0));
        })
        .catch((err) => {
          console.warn("Failed to load ticker grid state in modal:", err);
        });

      api
        .getVenueBalances()
        .then((balances) => {
          setTotalEquityUsd(resolveTotalPaperEquity(balances ?? []));
        })
        .catch(() => {
          setTotalEquityUsd(null);
        });
    }
  }, [isOpen, tickerSymbol, activeStrategyTab, paramsVersion]);

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

  const sizing = useMemo(() => {
    return deriveStepSizing(ticker.symbol, ticker.venues, ticker.basePrice, totalEquityUsd, {
      stepSizeMult,
      maxStepUsd,
      maxTickerEquityPct,
    });
  }, [ticker, stepSizeMult, maxStepUsd, maxTickerEquityPct, totalEquityUsd]);
  const unitSize = sizing.stepBaseQty;
  const maxAccumulatedUnits = sizing.maxSteps;

  const safeLeverage = useMemo(() => {
    return computeSafeLeverage(
      ticker.symbol,
      ticker.venues,
      ticker.basePrice,
      unitSize,
      userTargetLeverage
    );
  }, [ticker, unitSize, userTargetLeverage]);

  // Handle Escape key to close modal
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (autotuneOpen) {
          setAutotuneOpen(false);
        } else {
          onClose();
        }
      }
    };
    if (isOpen) {
      window.addEventListener("keydown", handleKeyDown);
    }
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [isOpen, onClose, autotuneOpen]);

  if (!isOpen) return null;

  const autotuneDisabledReason: string | null = !baselineSnapshot
    ? "Collecting baseline history…"
    : baselineSnapshot.max_history_span_secs < 86_400
      ? "AutoTune needs 24h+ of spread history"
      : baselineSnapshot.qualifying_pair_count === 0
        ? "No qualifying venue pairs — autoTune unavailable"
        : null;

  const handleOpenAutotune = () => {
    if (autotuneDisabledReason) return;
    setAutotuneOpen(true);
    setAutotuneLoading(true);
    setAutotuneError(null);
    setAutotuneRec(null);
    api
      .getAutotuneRecommendation(tickerSymbol)
      .then((rec) => setAutotuneRec(rec))
      .catch((err) => {
        setAutotuneError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => setAutotuneLoading(false));
  };

  const handleResetDefaults = () => {
    setTargetProfitBps(1.0);
    setWindowPercentile(50);
    setSlidingWindowMode("manual");
    setWindowValue(1);
    setWindowUnit("days");
    setCurrentAccumulatedUnits(0);
    setExecutionMode("SINGLE_WAY");
    setStepSizeMult(10);
    setMaxStepUsd(2500);
    setMaxTickerEquityPct(20);
    setUserTargetLeverage(5);
    setAutoMinVolumeUsd("");
    setAutoMinPriceUsd("");
    setAutoMinEventVolumeUsd(null);
    setFundingTradeSizeUsd(2000);
    setFundingMinAprPct(8.0);
    setFundingHoldingDays(14);
    setFundingMaxSlippageBps(15.0);
    setFundingMaxEntrySpreadBps(50.0);
    setFundingAutoExitSpreadPct("");
    setFundingRotationThresholdBps("");
    setFundingAutoRebalance(false);
    setFundingLiqTrimPct("");
    setFuturesMaxSlippageBps(15.0);
    setFuturesLiqTrimPct("");
    setSpotLiqTrimPct("");
    setSpotTradeSizeUsd(1000);
    setSpotMaxSlippageBps(5.0);
    setSpotMinNetSpreadBps(15.0);
    setTriangleTradeSizeUsd(10000);
    setTriangleMinProfitBps(2.0);
    setTriangleMaxSlippageBps(5.0);
    setFuturesPassive({ ...DEFAULT_PASSIVE_UNWIND });
    setFundingPassive({ ...DEFAULT_PASSIVE_UNWIND });
    setSpotPassive({ ...DEFAULT_PASSIVE_UNWIND });
    setFuturesPassiveMode("inherit");
    setFundingPassiveMode("inherit");
    setSpotPassiveMode("inherit");
  };

  const handleRevealAll = async () => {
    setRevealBusy(true);
    try {
      await api.revealAllTickers();
      setExemptList([]);
    } catch {
      /* leave list as-is on failure */
    } finally {
      setRevealBusy(false);
    }
  };

  const handleUnexempt = async (base: string) => {
    try {
      await api.setTickerExempted(base, false);
      setExemptList((l) => (l == null ? l : l.filter((b) => b !== base)));
    } catch {
      /* leave list as-is on failure */
    }
  };

  const parseAutoThreshold = (raw: string): number | null => {
    const v = parseFloat(raw);
    return Number.isFinite(v) && v > 0 ? v : null;
  };

  /** Shared Liquidation-Distance Trim block — `enforced` distinguishes
   *  strategies whose live supervisor consumes the threshold (futures)
   *  from those still waiting on a position loop (funding/spot). */
  const renderTrimBlock = (
    value: string,
    setValue: (v: string) => void,
    enforced: boolean,
  ) => (
    <SettingRow
      label="Liquidation-Distance Trim"
      description={
        enforced
          ? "Enforced live — checked every 5s on fresh position reads. When either leg comes within this percent of liquidation, both legs trim by the same qty; if the remainder drops below one step, both close. Empty = off."
          : "Saved but NOT YET ENFORCED — this strategy has no live position supervisor to consume the threshold yet. Planned behavior: when either leg comes within this percent of liquidation, both legs trim equally; below one step, both close. Empty = off."
      }
      tooltip="When EITHER leg comes within this percent of liquidation, BOTH legs trim by the same qty to stay neutral; if the remainder would drop below one step, both close instead. Empty = off."
    >
      <NumberInput
        value={value}
        onChange={setValue}
        unit="%"
        step="0.5"
        min="0"
        max="100"
        placeholder="OFF"
      />
    </SettingRow>
  );

  /** Shared Passive Limit Unwind block — per-strategy mode selects the
   *  config source: inherit the global policy, a custom override, or off. */
  const renderPassiveBlock = (
    value: PassiveUnwindDto,
    setValue: (v: PassiveUnwindDto) => void,
    mode: "inherit" | "custom" | "off",
    setMode: (m: "inherit" | "custom" | "off") => void,
  ) => {
    const set = (patch: Partial<PassiveUnwindDto>) => setValue({ ...value, ...patch });
    return (
      <div className="bg-surface p-4 rounded-xl border border-surface-border shadow-2xs space-y-3 transition hover:border-slate-300">
        <div className="flex items-center justify-between gap-3 pb-2 border-b border-surface-border/60">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <span className="text-xs font-semibold font-sans text-slate-800">Passive Limit Unwind</span>
              <InfoTooltip text="Close reduces with reduce-only LIMITs pegged inside the spread (0 = aggressive touch, 50 = mid, 100 = passive edge). Unfilled attempts wait timeoutSecs, re-price to a fresh book, and retry up to maxTries, then fall back to market. Skipped when spread or fast vol exceeds its cap. Inherit uses the global Engine Settings → Execution policy; Custom uses the params below; Off = market exits." />
            </div>
            <p className="text-[11px] font-sans text-slate-400 font-normal mt-0.5 leading-normal">
              timeout-bound · market fallback · emergency exits always market
            </p>
          </div>
          <div
            role="group"
            aria-label="Limit exit source"
            className="h-7 flex items-stretch rounded-lg border border-surface-border bg-surface overflow-hidden shrink-0"
          >
            {(["inherit", "custom", "off"] as const).map((m, i) => (
              <button
                key={m}
                type="button"
                aria-pressed={mode === m}
                onClick={() => setMode(m)}
                className={`px-2.5 text-[11px] font-mono font-bold transition cursor-pointer ${i > 0 ? "border-l border-surface-border" : ""} ${
                  mode === m ? "bg-accent text-white" : "text-slate-500 hover:text-slate-800 hover:bg-slate-100/70"
                }`}
              >
                {m === "inherit" ? "Global" : m === "custom" ? "Custom" : "Off"}
              </button>
            ))}
          </div>
        </div>

        {mode === "inherit" && (
          <p className="text-[11px] font-sans text-slate-400 leading-normal">
            Follows the global policy in Engine Settings → Execution → Passive Limit Unwind.
          </p>
        )}
        {mode === "off" && (
          <p className="text-[11px] font-sans text-slate-400 leading-normal">
            This strategy always exits at market — the limit-exit method is never used for it.
          </p>
        )}

        {mode === "custom" && (
        <>
        <div className="flex items-center justify-between gap-3">
          <span className="text-xs font-medium font-sans text-slate-700">Enable custom limit exits</span>
          <SwitchToggle
            size="sm"
            checked={value.enabled}
            ariaLabel="Passive limit unwind enabled"
            onChange={(on) => set({ enabled: on })}
          />
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5 pt-1">
          <div className="flex items-center justify-between gap-2 p-2.5 rounded-lg bg-surface-raised/60 border border-surface-border/60">
            <div className="min-w-0 flex-1">
              <span className="text-xs font-medium font-sans text-slate-700 block">Spread Percentile</span>
              <span className="text-[10px] font-sans text-slate-400">0 aggressive · 50 mid · 100 edge</span>
            </div>
            <NumberInput
              value={value.spreadPercentile}
              onChange={(raw) => {
                const n = parseFloat(raw);
                if (Number.isFinite(n)) set({ spreadPercentile: Math.max(0, Math.min(100, n)) });
              }}
              unit="%"
              step="5"
              min="0"
              max="100"
              disabled={!value.enabled}
              className="w-24"
            />
          </div>

          <div className="flex items-center justify-between gap-2 p-2.5 rounded-lg bg-surface-raised/60 border border-surface-border/60">
            <div className="min-w-0 flex-1">
              <span className="text-xs font-medium font-sans text-slate-700 block">Limit Timeout</span>
              <span className="text-[10px] font-sans text-slate-400">Wait per unfilled try before re-pricing</span>
            </div>
            <NumberInput
              value={value.timeoutSecs}
              onChange={(raw) => {
                const n = parseFloat(raw);
                if (Number.isFinite(n)) set({ timeoutSecs: Math.max(0, Math.min(300, Math.round(n))) });
              }}
              unit="s"
              step="1"
              min="0"
              max="300"
              disabled={!value.enabled}
              className="w-24"
            />
          </div>

          <div className="flex items-center justify-between gap-2 p-2.5 rounded-lg bg-surface-raised/60 border border-surface-border/60">
            <div className="min-w-0 flex-1">
              <span className="text-xs font-medium font-sans text-slate-700 block">Max Limit Tries</span>
              <span className="text-[10px] font-sans text-slate-400">Fallback to market after attempts</span>
            </div>
            <NumberInput
              value={value.maxTries}
              onChange={(raw) => {
                const n = parseFloat(raw);
                if (Number.isFinite(n)) set({ maxTries: Math.max(1, Math.min(10, Math.round(n))) });
              }}
              unit="tries"
              step="1"
              min="1"
              max="10"
              disabled={!value.enabled}
              className="w-24"
            />
          </div>

          <div className="flex items-center justify-between gap-2 p-2.5 rounded-lg bg-surface-raised/60 border border-surface-border/60">
            <div className="min-w-0 flex-1">
              <span className="text-xs font-medium font-sans text-slate-700 block">Max Spread Cap</span>
              <span className="text-[10px] font-sans text-slate-400">Skip passive above live spread</span>
            </div>
            <NumberInput
              value={value.maxSpreadBps}
              onChange={(raw) => {
                const n = parseFloat(raw);
                if (Number.isFinite(n)) set({ maxSpreadBps: Math.max(0.1, n) });
              }}
              unit="bps"
              step="1"
              min="0.1"
              disabled={!value.enabled}
              className="w-24"
            />
          </div>

          <div className="flex items-center justify-between gap-2 p-2.5 rounded-lg bg-surface-raised/60 border border-surface-border/60 sm:col-span-2">
            <div className="min-w-0 flex-1">
              <span className="text-xs font-medium font-sans text-slate-700 block">Max Fast Volatility Cap</span>
              <span className="text-[10px] font-sans text-slate-400">Skip passive above fast volatility</span>
            </div>
            <NumberInput
              value={value.maxFastVolBps}
              onChange={(raw) => {
                const n = parseFloat(raw);
                if (Number.isFinite(n)) set({ maxFastVolBps: Math.max(0.1, n) });
              }}
              unit="bps"
              step="1"
              min="0.1"
              disabled={!value.enabled}
              className="w-24"
            />
          </div>
        </div>
        </>
        )}
      </div>
    );
  };

  /** Shared Visibility block (Auto-hide filters + Shared Manual Hide list) */
  const renderVisibilitySection = () => (
    <div className="space-y-3">
      <SettingRow
        label="Auto Min 24h Volume"
        description="Automatic · MIN across in-scope venues defeats single-venue wash inflation · empty = off"
        tooltip="Hide tickers whose MINIMUM 24h volume across in-scope venues falls below this floor. Any venue missing volume data excludes the ticker (fail-closed). Empty = off."
      >
        <NumberInput
          value={autoMinVolumeUsd}
          onChange={setAutoMinVolumeUsd}
          unit="USD"
          step="1000"
          min="0"
          placeholder="OFF"
        />
      </SettingRow>

      <SettingRow
        label="Auto Min Mid Price"
        description="Automatic · minimum mid across in-scope venues · empty = off"
        tooltip="Hide tickers whose minimum mid price across in-scope venues falls below this floor. Any venue missing a mid excludes the ticker (fail-closed). Empty = off."
      >
        <NumberInput
          value={autoMinPriceUsd}
          onChange={setAutoMinPriceUsd}
          unit="USD"
          step="0.01"
          min="0"
          placeholder="OFF"
        />
      </SettingRow>

      <div className="bg-surface p-4 rounded-xl border border-surface-border space-y-3 shadow-2xs">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5 min-w-0">
            <span className="text-xs font-semibold font-sans text-slate-800 block">Hidden Tickers</span>
            <p className="text-[11px] font-sans text-slate-500">
              {exemptList === null
                ? "Hidden tickers are excluded from opportunity scanners and radar views."
                : exemptList.length === 0
                  ? "No tickers are currently hidden."
                  : `${exemptList.length} ticker${exemptList.length === 1 ? "" : "s"} hidden.`}
            </p>
            <p className="text-[10px] font-sans text-slate-400">Shared list · applies to all strategies after auto-filtering</p>
          </div>
          <button
            type="button"
            disabled={revealBusy || !exemptList?.length}
            onClick={() => void handleRevealAll()}
            className="h-8 px-2 rounded-lg border border-surface-border text-xs font-mono font-semibold text-slate-600 hover:text-accent hover:border-accent/40 bg-surface-raised transition cursor-pointer disabled:opacity-40 shrink-0"
          >
            {revealBusy ? "…" : "Reveal All"}
          </button>
        </div>
        {exemptList && exemptList.length > 0 && (
          <div className="flex flex-wrap gap-1.5 pt-1">
            {exemptList.map((base) => (
              <button
                key={base}
                type="button"
                onClick={() => void handleUnexempt(base)}
                title={`Un-hide ${base}`}
                className="flex items-center gap-1.5 px-2 py-2 rounded-lg border border-surface-border bg-surface-raised text-xs font-mono font-semibold text-slate-700 hover:text-accent hover:border-accent/40 transition cursor-pointer"
              >
                <span>{base}</span>
                <span className="text-slate-400 hover:text-loss text-xs">×</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );

  /** Futures Signal Trigger Section */
  const renderFuturesHurdleSection = () => (
    <div className="space-y-3">
      <div className="flex items-center justify-between pb-1 border-b border-surface-border/60">
        <div>
          <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Signal Trigger & Timing</h3>
          <p className="text-[11px] font-sans text-slate-400 mt-0.5">Thresholds and historical baseline models</p>
        </div>
        <button
          type="button"
          onClick={handleOpenAutotune}
          disabled={autotuneDisabledReason != null}
          title={autotuneDisabledReason ?? "Walk-forward AutoTune preview (70/30 split; selected READY rows can be applied)"}
          className="px-2 py-2 rounded-lg font-mono text-[11px] font-semibold border transition cursor-pointer disabled:cursor-not-allowed bg-accent-dim border-accent-line text-accent hover:bg-blue-100 disabled:bg-surface-raised disabled:border-surface-border disabled:text-slate-400 shadow-2xs"
        >
          AutoTune
        </button>
      </div>

      <SettingRow
        label="Min Net Profit"
        description="Net profit cushion locked in per round-trip after taker fees and closing spread"
        tooltip="Net profit cushion locked in per round-trip after deducting taker fees and closing spread."
      >
        <NumberInput
          value={targetProfitBps}
          onChange={(v) => setTargetProfitBps(Math.max(0.1, parseFloat(v) || 0.1))}
          unit="bps"
          step="0.5"
          min="0.1"
          max="100"
        />
      </SettingRow>

      <SettingRow
        label="Max Slippage"
        description="Entries whose VWAP slips beyond this tolerance are refused"
        tooltip="Per-strategy slippage gate: entries whose VWAP slips beyond this tolerance are refused."
      >
        <NumberInput
          value={futuresMaxSlippageBps}
          onChange={(v) => setFuturesMaxSlippageBps(Math.max(0.1, parseFloat(v) || 15))}
          unit="bps"
          step="0.5"
          min="0.1"
        />
      </SettingRow>

      <div className="bg-surface p-3.5 rounded-xl border border-surface-border space-y-2.5 shadow-2xs transition hover:border-slate-300">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-semibold font-sans text-slate-800">Sliding Window</span>
            <InfoTooltip text="Trailing duration for the mean-spread baseline and percentile trigger. Auto derives the window per ticker from mean-reversion half-life; manual duration stays as the explicit fallback." />
          </div>
          <SwitchToggle
            size="sm"
            label="Auto from history"
            checked={slidingWindowMode === "auto"}
            onChange={(v) => setSlidingWindowMode(v ? "auto" : "manual")}
          />
        </div>
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
            className="h-9 w-full px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono font-bold text-slate-900 tabular-nums focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition"
          />
          <select
            value={windowUnit}
            onChange={(e) => switchWindowUnit(e.target.value as DurationUnit)}
            className="h-9 w-32 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-sans text-xs font-medium focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition cursor-pointer"
          >
            <option value="minutes">Minutes</option>
            <option value="hours">Hours</option>
            <option value="days">Days</option>
          </select>
        </div>
        <div className="text-[11px] font-sans text-slate-400">{windowStatusLine}</div>
      </div>

      <SessionProfileCard symbol={tickerSymbol} />

      <SettingRow
        label="Window Percentile"
        description="Strictly filters entries to spreads at or above this percentile of rolling window"
        tooltip="Strictly filters entries to spreads at or above this percentile of the rolling sliding window."
      >
        <select
          value={windowPercentile}
          onChange={(e) => setWindowPercentile(Math.max(50, parseInt(e.target.value, 10)))}
          className="h-9 w-44 shrink-0 px-3 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-slate-900 font-sans text-xs font-medium focus:outline-none focus:ring-2 focus:ring-accent/20 focus:border-accent transition cursor-pointer"
        >
          {PERCENTILE_OPTIONS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
        </select>
      </SettingRow>

      <div className="bg-surface p-3.5 rounded-xl border border-surface-border space-y-2 shadow-2xs transition hover:border-slate-300">
        <div className="flex items-center gap-1.5">
          <span className="text-xs font-semibold font-sans text-slate-800">Execution Mode</span>
          <InfoTooltip text="Two-Way: enter on divergence and auto-unwind when the spread converges to zero. Single-Way: entry only — inventory held until manual exit." />
        </div>
        <div className="grid grid-cols-2 gap-2 bg-surface-raised p-1 rounded-lg border border-surface-border text-xs">
          <button
            type="button"
            onClick={() => setExecutionMode("SINGLE_WAY")}
            className={`w-full py-2 px-2 rounded-lg font-sans text-xs font-semibold transition cursor-pointer ${
              executionMode === "SINGLE_WAY"
                ? "bg-accent text-white shadow-2xs"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Single-Way: Enter on wide divergence only"
          >
            Single-Way
          </button>
          <button
            type="button"
            onClick={() => setExecutionMode("DOUBLE_WAY")}
            className={`w-full py-2 px-2 rounded-lg font-sans text-xs font-semibold transition cursor-pointer ${
              executionMode === "DOUBLE_WAY"
                ? "bg-accent text-white shadow-2xs"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Two-Way: enter on divergence and automatically unwind when the spread converges to zero"
          >
            Two-Way
          </button>
        </div>
      </div>
    </div>
  );

  /** Futures Sizing Section */
  const renderFuturesSizingSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Execution & Sizing</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Step size, lot bounds, and capital allocation</p>
      </div>

      <SettingRow
        label="Step Multiplier"
        description="Lot multiplier scaling the venue-derived minimum contract size (≥ 1)"
        tooltip="Derives the minimum order size as a multiple of the cross-venue common lot step."
      >
        <NumberInput
          value={stepSizeMult}
          onChange={(v) => setStepSizeMult(Math.max(1, parseFloat(v) || 10))}
          unit="× min lot"
          step="1"
          min="1"
        />
      </SettingRow>

      <SettingRow
        label="Max Step Cap"
        description="Hard upper bound on dollar notional per single order step"
        tooltip="Absolute ceiling for notional allocation on any individual grid step."
      >
        <NumberInput
          value={maxStepUsd}
          onChange={(v) => setMaxStepUsd(Math.max(10, parseFloat(v) || 2500))}
          unit="USD"
          step="100"
          min="10"
        />
      </SettingRow>

      <SettingRow
        label="Max Ticker Equity"
        description="Maximum total portfolio exposure percentage committed to this ticker"
        tooltip="Caps total accumulated grid position sizing across all legs of this ticker to a fraction of total portfolio equity."
      >
        <NumberInput
          value={maxTickerEquityPct}
          onChange={(v) => setMaxTickerEquityPct(Math.max(0.1, Math.min(100, parseFloat(v) || 20)))}
          unit="%"
          step="0.5"
          min="0.1"
          max="100"
        />
      </SettingRow>
    </div>
  );

  /** Futures Leverage & Margin Section */
  const renderFuturesLeverageSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Leverage & Margin Safety</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Per-venue margin caps, liquidation buffers, and trim triggers</p>
      </div>

      <SettingRow
        label="Target Leverage"
        description="Requested strategy leverage, default 5x (effective is min(strategy, Risk Control max, account, venue))"
        tooltip="Requested leverage tier for opening perpetual positions, up to 10x. Effective leverage is min(strategy target, Risk Control global max, account max, venue max)."
      >
        <NumberInput
          value={userTargetLeverage}
          onChange={(v) => setUserTargetLeverage(Math.min(10, Math.max(1, parseInt(v, 10) || 5)))}
          unit="x"
          step="1"
          min="1"
          max="10"
        />
      </SettingRow>

      <div className="grid grid-cols-2 gap-3">
        <div className="bg-surface p-3.5 rounded-xl border border-surface-border space-y-1 shadow-2xs">
          <span className="text-[11px] font-sans text-slate-400 font-medium">Effective Safe Leverage</span>
          <div className="text-base font-bold font-mono text-blue-700">{safeLeverage.effectiveLeverage}x</div>
          <span className="text-[10px] font-sans text-slate-400 block">Max venue: {safeLeverage.maxVenueLeverage}x</span>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border space-y-1 shadow-2xs">
          <span className="text-[11px] font-sans text-slate-400 font-medium">Liquidation Buffer</span>
          <div className="text-base font-bold font-mono tabular-nums text-profit">
            {safeLeverage.liquidationBufferPct.toFixed(1)}%
          </div>
          <span className="text-[10px] font-sans text-slate-400 block">Dual-leg safety margin</span>
        </div>
      </div>

      {renderTrimBlock(futuresLiqTrimPct, setFuturesLiqTrimPct, true)}
    </div>
  );

  /** Funding Carry Parameters Section */
  const renderFundingCarrySection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Carry Parameters</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Position sizing and target return hurdle</p>
      </div>

      <SettingRow
        label="Trade Size"
        description="Notional capital deployed per carry position leg"
        tooltip="Target dollar value for each leg of the delta-neutral funding rate position."
      >
        <NumberInput
          value={fundingTradeSizeUsd}
          onChange={(v) => setFundingTradeSizeUsd(Math.max(100, parseFloat(v) || 1000))}
          unit="USD"
          step="1000"
          min="1000"
        />
      </SettingRow>

      <SettingRow
        label="Min APR Hurdle"
        description="Ignore funding rate opportunities below this net annualized yield"
        tooltip="Minimum net APR after deducting continuous borrow interest, taker fees, and amortized execution friction."
      >
        <NumberInput
          value={fundingMinAprPct}
          onChange={(v) => setFundingMinAprPct(Math.max(0.1, parseFloat(v) || 1))}
          unit="%"
          step="0.5"
          min="1"
        />
      </SettingRow>

      <SettingRow
        label="Basis Trades"
        description="Include same-exchange long-spot / short-perp cash-and-carry in the scanner"
        tooltip="When ON, venues with a live funding print and both spot + perp books emit basis opportunities alongside cross-exchange carry. Reverse (long perp / short spot) needs borrow and is never emitted."
      >
        <SwitchToggle
          size="sm"
          checked={fundingEnableBasis}
          ariaLabel="Basis trades enabled"
          onChange={setFundingEnableBasis}
        />
      </SettingRow>

      <SettingRow
        label="Min Basis"
        description="Skip basis opps below this signed spot/perp divergence · empty = off"
        tooltip="Floor on (perp - spot) / spot in bps. Positive = contango only; empty disables the floor (funding APR still gates)."
      >
        <NumberInput
          value={fundingMinBasisBps}
          onChange={setFundingMinBasisBps}
          unit="bps"
          step="1"
          placeholder="OFF"
        />
      </SettingRow>

    </div>
  );

  /** Funding Execution & Slippage Section */
  const renderFundingExecutionSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Execution & Slippage</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Orderbook tolerance limits and convergence exit triggers</p>
      </div>

      <SettingRow
        label="Max Slippage"
        description="Per-strategy slippage gate for funding entries"
        tooltip="Individual leg VWAP slippage ceiling. Entries exceeding this tolerance are aborted."
      >
        <NumberInput
          value={fundingMaxSlippageBps}
          onChange={(v) => setFundingMaxSlippageBps(Math.max(0.1, parseFloat(v) || 15))}
          unit="bps"
          step="0.5"
          min="0.1"
        />
      </SettingRow>

      <SettingRow
        label="Max Entry Spread"
        description="Refuse entry when the venue execution-price gap exceeds this limit"
        tooltip="Prevents entering carry trades when the immediate spot-perp basis is too wide and adverse."
      >
        <NumberInput
          value={fundingMaxEntrySpreadBps}
          onChange={(v) => setFundingMaxEntrySpreadBps(Math.max(0.1, parseFloat(v) || 50))}
          unit="bps"
          step="1"
          min="0.1"
        />
      </SettingRow>

      <SettingRow
        label="Auto-Exit Spread"
        description="Exit early when inter-venue spread drops below this % · empty = off"
        tooltip="Automatically unwinds position if the price spread closes faster than the holding horizon."
      >
        <NumberInput
          value={fundingAutoExitSpreadPct}
          onChange={setFundingAutoExitSpreadPct}
          unit="%"
          step="0.5"
          min="0"
          max="100"
          placeholder="OFF"
        />
      </SettingRow>
    </div>
  );

  /** Funding Risk & Rebalance Section */
  const renderFundingRiskSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Risk & Rebalance</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Drift rebalancing, rotation thresholds, and margin trim protection</p>
      </div>

      {renderTrimBlock(fundingLiqTrimPct, setFundingLiqTrimPct, false)}

      <SettingRow
        label="Auto-Rebalance to Neutral"
        description="Trims the overweight leg back to neutral when the pair drifts"
        tooltip="Reduces the overweight leg back to neutral when execution lot mismatches or partial fills cause delta exposure."
      >
        <SwitchToggle
          size="sm"
          checked={fundingAutoRebalance}
          ariaLabel="Auto-rebalance to neutral enabled"
          onChange={setFundingAutoRebalance}
        />
      </SettingRow>

      <SettingRow
        label="Rotation Threshold"
        description="Rotate only when net-of-friction APR improvement exceeds this · empty = off"
        tooltip="Minimum APR improvement required over the current position before switching to a higher-yielding pair."
      >
        <NumberInput
          value={fundingRotationThresholdBps}
          onChange={setFundingRotationThresholdBps}
          unit="bps"
          step="1"
          min="0"
          placeholder="OFF"
        />
      </SettingRow>
    </div>
  );

  /** Spot Spatial Parameters Section */
  const renderSpotSpatialSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Spatial Spot Parameters</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Capital per flash cycle, net spread hurdles, and slippage</p>
      </div>

      <SettingRow
        label="Trade Size"
        description="Capital deployed per spatial flash cycle"
        tooltip="Notional USD capital deployed across paired spot exchange books."
      >
        <NumberInput
          value={spotTradeSizeUsd}
          onChange={(v) => setSpotTradeSizeUsd(Math.max(10, parseFloat(v) || 500))}
          unit="USD"
          step="100"
          min="100"
        />
      </SettingRow>

      <SettingRow
        label="Min Net Spread"
        description="Minimum net spread hurdle required after deducting fees to auto-execute"
        tooltip="Required price differential between buy and sell venues net of all exchange taker fees and withdrawal costs."
      >
        <NumberInput
          value={spotMinNetSpreadBps}
          onChange={(v) => setSpotMinNetSpreadBps(Math.max(0.1, parseFloat(v) || 1))}
          unit="bps"
          step="0.5"
          min="0.1"
        />
      </SettingRow>

      <SettingRow
        label="Max Slippage"
        description="Orderbook depth tolerance limit across buy and sell venues"
        tooltip="Execution VWAP slippage tolerance. Trades exceeding this limit are rejected."
      >
        <NumberInput
          value={spotMaxSlippageBps}
          onChange={(v) => setSpotMaxSlippageBps(Math.max(0.1, parseFloat(v) || 5))}
          unit="bps"
          step="0.5"
          min="0.5"
          max="50"
        />
      </SettingRow>
    </div>
  );

  /** Spot Risk & Safety Section */
  const renderSpotRiskSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Risk & Safety</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Liquidation-distance trim protection</p>
      </div>

      {renderTrimBlock(spotLiqTrimPct, setSpotLiqTrimPct, false)}
    </div>
  );

  /** Triangle Cycle Parameters Section */
  const renderPredictionParitySection = () => (
    <div className="space-y-3">
      <div className="flex items-center justify-between pb-1 border-b border-surface-border/60">
        <div>
          <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Parity Parameters</h3>
          <p className="text-[11px] font-sans text-slate-400 mt-0.5">Minimum net profit and disabled scaffold gates</p>
        </div>
      </div>

      <SettingRow
        label="Min Net Profit"
        description="Minimum net profit required to auto-execute a parity pair"
        tooltip="Prediction parity auto-executes only when net profit reaches this threshold."
      >
        <span className="font-sans font-bold text-slate-900 tabular-nums text-xs">1.0 ¢</span>
      </SettingRow>

      <div className="p-3.5 bg-surface-raised border border-surface-border rounded-lg space-y-2 text-xs font-sans text-slate-500 leading-relaxed">
        <p><strong className="text-slate-700">Min Vol</strong> — defined but disabled: no venue volume data (prediction adapters are scaffolds).</p>
        <p><strong className="text-slate-700">Max Slip</strong> — defined but disabled: no execution path (prediction adapters are scaffolds).</p>
      </div>
    </div>
  );

  const renderTriangleCycleSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Cycle Parameters</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Triangular loop capital and round-trip profit hurdle</p>
      </div>

      <SettingRow
        label="Trade Size"
        description="Standard capital allocated per 3-step triangular cycle"
        tooltip="Initial capital deployed across the first leg of the synthetic triangular cycle."
      >
        <NumberInput
          value={triangleTradeSizeUsd}
          onChange={(v) => setTriangleTradeSizeUsd(Math.max(10, parseFloat(v) || 1000))}
          unit="USD"
          step="100"
          min="10"
        />
      </SettingRow>

      <SettingRow
        label="Min Net Profit"
        description="Minimum net edge required after deducting trading fees across all 3 legs"
        tooltip="Refuse triangular arbitrage cycles whose combined net edge after 3-leg taker fees and bid/ask spreads is below this hurdle."
      >
        <NumberInput
          value={triangleMinProfitBps}
          onChange={(v) => setTriangleMinProfitBps(Math.max(0.1, parseFloat(v) || 0.1))}
          unit="bps"
          step="0.5"
          min="0.1"
          max="100"
        />
      </SettingRow>
    </div>
  );

  /** Triangle Cycle Guardrails Section */
  const renderTriangleGuardrailsSection = () => (
    <div className="space-y-3">
      <div className="pb-1 border-b border-surface-border/60">
        <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Cycle Guardrails</h3>
        <p className="text-[11px] font-sans text-slate-400 mt-0.5">Per-leg slippage gates and orderbook execution controls</p>
      </div>

      <SettingRow
        label="Max Leg Slippage"
        description="Orderbook depth tolerance limit for each leg of the triangle"
        tooltip="Individual leg VWAP slippage ceiling. If any single leg slips beyond this tolerance, the cycle is aborted."
      >
        <NumberInput
          value={triangleMaxSlippageBps}
          onChange={(v) => setTriangleMaxSlippageBps(Math.max(0.1, parseFloat(v) || 1))}
          unit="bps"
          step="0.5"
          min="0.1"
          max="50"
        />
      </SettingRow>
    </div>
  );

  const handleSave = async () => {
    setIsSaving(true);
    setSaveSuccess(false);
    setSaveError(null);

    const canonicalWindow = formatDuration(manualWindowSecs);
    const currentRecord = await api.getStrategyParams(activeStrategyTab).catch(() => null);
    const updatedParams: StrategyParamsDto = {
      strategyId: activeStrategyTab,
      targetProfitBps,
      windowPercentile,
      slidingWindow: canonicalWindow,
      slidingWindowMode,
      slidingWindowSecs: manualWindowSecs,
      stepSizeMult,
      maxStepUsd,
      maxTickerEquityPct,
      executionMode,
      userTargetLeverage,
      autoMinVolumeUsd: parseAutoThreshold(autoMinVolumeUsd),
      autoMinPriceUsd: parseAutoThreshold(autoMinPriceUsd),
      autoMinEventVolumeUsd,
      fundingTradeSizeUsd,
      fundingMinAprPct,
      fundingHoldingDays,
      futuresMaxSlippageBps,
      fundingMaxSlippageBps,
      predictionMaxSlippageBps: currentRecord?.predictionMaxSlippageBps ?? 15.0,
      fundingMaxEntrySpreadBps,
      fundingAutoExitSpreadPct: parseOptThreshold(fundingAutoExitSpreadPct),
      fundingLiqTrimPct: parseOptThreshold(fundingLiqTrimPct),
      fundingAutoRebalance,
      fundingRotationThresholdBps: parseOptThreshold(fundingRotationThresholdBps),
      fundingEnableBasis,
      fundingMinBasisBps: fundingMinBasisBps.trim() === "" ? null : Number(fundingMinBasisBps),
      futuresLiqTrimPct: parseOptThreshold(futuresLiqTrimPct),
      spotLiqTrimPct: parseOptThreshold(spotLiqTrimPct),
      predictionLiqTrimPct: currentRecord?.predictionLiqTrimPct ?? null,
      spotTradeSizeUsd,
      spotMaxSlippageBps,
      spotMinNetSpreadBps,
      triangleTradeSizeUsd,
      triangleMinProfitBps,
      triangleMaxSlippageBps,
      futuresPassiveUnwind:
        activeStrategyTab === "futures" ? futuresPassive : (currentRecord?.futuresPassiveUnwind ?? { ...DEFAULT_PASSIVE_UNWIND }),
      fundingPassiveUnwind:
        activeStrategyTab === "funding" ? fundingPassive : (currentRecord?.fundingPassiveUnwind ?? { ...DEFAULT_PASSIVE_UNWIND }),
      spotPassiveUnwind:
        activeStrategyTab === "spot" ? spotPassive : (currentRecord?.spotPassiveUnwind ?? { ...DEFAULT_PASSIVE_UNWIND }),
      futuresPassiveMode:
        activeStrategyTab === "futures" ? futuresPassiveMode : (currentRecord?.futuresPassiveMode ?? "inherit"),
      fundingPassiveMode:
        activeStrategyTab === "funding" ? fundingPassiveMode : (currentRecord?.fundingPassiveMode ?? "inherit"),
      spotPassiveMode:
        activeStrategyTab === "spot" ? spotPassiveMode : (currentRecord?.spotPassiveMode ?? "inherit"),
    };

    const validationErrors = validateStrategyParams(updatedParams);
    if (validationErrors.length > 0) {
      setSaveError(validationErrors[0]);
      setIsSaving(false);
      return;
    }

    try {
      await api.saveStrategyParams(updatedParams);
      if (activeStrategyTab === "futures") {
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
      }
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
      // Tauri command rejections arrive as plain strings (not Error
      // instances) — render them verbatim so the backend's field-level
      // reason (e.g. which bound failed) is visible instead of a dead-end
      // generic message.
      setSaveError(
        err instanceof Error
          ? err.message
          : typeof err === "string" && err.trim() !== ""
            ? err
            : "Save failed — parameters were not written."
      );
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none animate-in fade-in duration-150"
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
            <div className="w-8 h-8 rounded-lg bg-accent-dim border border-accent-line flex items-center justify-center text-accent shadow-2xs">
              <SlidersHorizontal className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-bold font-sans tracking-tight text-slate-900 leading-tight">
                {formatStrategyName(activeStrategyTab)} Configurator
              </h2>
              <p className="text-[11px] font-sans text-slate-500">
                Strategy execution limits, spread thresholds, and risk gates
              </p>
            </div>
          </div>

          <div className="flex items-center space-x-3">
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition border border-transparent hover:border-surface-border cursor-pointer"
              title="Close Configurator (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Modal Middle Body: Left Sidebar + Right Content Area */}
        <div className="flex-1 flex min-h-0 overflow-hidden">
          {/* Left Vertical Strategy Selector Panel */}
          <div className="w-56 shrink-0 border-r border-surface-border bg-slate-50/70 p-3 flex flex-col justify-between select-none overflow-y-auto">
            <div className="space-y-3">
              <div className="px-2 pt-1 flex items-center justify-between">
                <span className="text-[11px] font-sans font-bold tracking-wider uppercase text-slate-400">
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
                      className={`w-full flex items-center space-x-2.5 px-2.5 py-2.5 rounded-lg font-sans text-xs transition cursor-pointer text-left ${
                        activeSection === s.id
                          ? "bg-accent text-white font-bold shadow-2xs border border-accent"
                          : "text-slate-600 hover:text-slate-900 bg-surface hover:bg-slate-100/80 border border-surface-border/60 hover:border-surface-border"
                      }`}
                    >
                      <Icon className={`w-3.5 h-3.5 shrink-0 ${activeSection === s.id ? "text-white" : "text-slate-400"}`} />
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
          <div className="flex-1 overflow-y-auto p-5 space-y-4 bg-surface">
            {/* Universal Sections: Routing and Visibility */}
            {activeSection === "routing" ? (
              <div className="space-y-3">
                {routingError && (
                  <div className="p-3 bg-loss-dim border border-loss-border rounded-lg text-xs font-mono text-loss">
                    {routingError}
                  </div>
                )}
                <StrategyRouteCard
                  strategy={
                    STRATEGIES.find((s) => s.id === activeStrategyTab) ?? {
                      id: activeStrategyTab,
                      name: formatStrategyName(activeStrategyTab),
                      desc: `Routing for ${formatStrategyName(activeStrategyTab)} strategy`,
                    }
                  }
                  routes={routes.filter((r) => r.strategyId === activeStrategyTab)}
                  accounts={routeAccounts}
                  onChanged={() => void reloadRoutes()}
                />
              </div>
            ) : activeSection === "visibility" ? (
              renderVisibilitySection()
            ) : activeSection === "unwind" ? (
              <div className="space-y-3">
                <div className="pb-1 border-b border-surface-border/60">
                  <h3 className="text-xs font-bold font-sans text-slate-800 tracking-tight">Passive Unwind Execution</h3>
                  <p className="text-[11px] font-sans text-slate-400 mt-0.5">Pegged limit order execution with market fallbacks</p>
                </div>
                {activeStrategyTab === "futures" && renderPassiveBlock(futuresPassive, setFuturesPassive, futuresPassiveMode, setFuturesPassiveMode)}
                {activeStrategyTab === "funding" && renderPassiveBlock(fundingPassive, setFundingPassive, fundingPassiveMode, setFundingPassiveMode)}
                {activeStrategyTab === "spot" && renderPassiveBlock(spotPassive, setSpotPassive, spotPassiveMode, setSpotPassiveMode)}
              </div>
            ) : (
              /* Strategy-Specific Sections */
              <>
                {/* Simple Spread (Futures) */}
                {activeStrategyTab === "futures" && (
                  <>
                    {activeSection === "hurdle" && renderFuturesHurdleSection()}
                    {activeSection === "sizing" && renderFuturesSizingSection()}
                    {activeSection === "leverage" && renderFuturesLeverageSection()}
                  </>
                )}

                {/* Funding Carry */}
                {activeStrategyTab === "funding" && (
                  <>
                    {activeSection === "carry" && renderFundingCarrySection()}
                    {activeSection === "execution" && renderFundingExecutionSection()}
                    {activeSection === "risk" && renderFundingRiskSection()}
                  </>
                )}

                {/* Spatial Spot */}
                {activeStrategyTab === "spot" && (
                  <>
                    {activeSection === "spatial" && renderSpotSpatialSection()}
                    {activeSection === "risk" && renderSpotRiskSection()}
                  </>
                )}

                {/* Triangular Arbitrage */}
                {activeStrategyTab === "triangle" && (
                  <>
                    {activeSection === "cycle" && renderTriangleCycleSection()}
                    {activeSection === "guardrails" && renderTriangleGuardrailsSection()}
                  </>
                )}

                {/* Prediction Market Parity */}
                {activeStrategyTab === "prediction" && (
                  <>
                    {activeSection === "parity" && renderPredictionParitySection()}
                  </>
                )}
              </>
            )}
          </div>
        </div>

        {/* Modal Bottom Footer Actions */}
        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-between shrink-0 font-sans text-xs">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="flex items-center space-x-1.5 px-2 py-2 bg-surface hover:bg-surface-raised text-slate-600 hover:text-slate-900 rounded-lg border border-surface-border transition cursor-pointer font-medium"
            title="Reset to default quantitative parameters"
          >
            <RotateCcw className="w-3.5 h-3.5 text-slate-500" />
            <span>Reset Defaults</span>
          </button>

          <div className="flex items-center space-x-3">
            {saveError && (
              <span role="alert" className="text-xs font-semibold text-loss max-w-60 text-right">
                {saveError}
              </span>
            )}
            <button
              type="button"
              onClick={onClose}
              className="px-2.5 py-2.5 bg-surface hover:bg-surface-raised text-slate-700 border border-surface-border rounded-lg font-medium transition cursor-pointer"
            >
              Cancel
            </button>

            <button
              type="button"
              onClick={handleSave}
              disabled={isSaving}
              className={`flex items-center space-x-1.5 px-2.5 py-2.5 rounded-lg font-semibold transition shadow-2xs cursor-pointer ${
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

        {autotuneOpen && (
          <AutotunePreviewModal
            symbol={tickerSymbol}
            recommendation={autotuneRec}
            loading={autotuneLoading}
            error={autotuneError}
            onClose={() => setAutotuneOpen(false)}
            onApplied={() => {
              setParamsVersion((v) => v + 1);
              setAutotuneLoading(true);
              setAutotuneError(null);
              api
                .getAutotuneRecommendation(tickerSymbol)
                .then((rec) => setAutotuneRec(rec))
                .catch((err) => {
                  setAutotuneError(err instanceof Error ? err.message : String(err));
                })
                .finally(() => setAutotuneLoading(false));
            }}
          />
        )}
      </div>
    </div>
  );
};
