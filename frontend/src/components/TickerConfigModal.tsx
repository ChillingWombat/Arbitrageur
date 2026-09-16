import React, { useState, useEffect, useMemo } from "react";
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
} from "lucide-react";
import type { TickerStrategyConfigDto, StrategyParamsDto, PassiveUnwindDto, ArbitrageExecutionMode, MatrixBaselineSnapshotDto, AutotuneRecommendationDto } from "../types";
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
    { id: "visibility", label: "Ticker Visibility", tooltip: "Automatic per-strategy hide filters (MIN across venues, fail-closed) plus the shared manual hide list, which applies after auto-filtering.", icon: EyeOff },
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
  const [autotuneOpen, setAutotuneOpen] = useState(false);
  const [autotuneRec, setAutotuneRec] = useState<AutotuneRecommendationDto | null>(null);
  const [autotuneLoading, setAutotuneLoading] = useState(false);
  const [autotuneError, setAutotuneError] = useState<string | null>(null);
  // Bumped after a one-click autotune apply so the form reloads the
  // freshly written strategy params through the same load path as open.
  const [paramsVersion, setParamsVersion] = useState(0);
  const [currentAccumulatedUnits, setCurrentAccumulatedUnits] = useState<number>(0);
  const [executionMode, setExecutionMode] = useState<ArbitrageExecutionMode>("SINGLE_WAY");
  const [stepSizeMult, setStepSizeMult] = useState<number>(10);
  const [maxStepUsd, setMaxStepUsd] = useState<number>(2500);
  const [maxTickerEquityPct, setMaxTickerEquityPct] = useState<number>(20);
  const [totalEquityUsd, setTotalEquityUsd] = useState<number | null>(null);
  const [userTargetLeverage, setUserTargetLeverage] = useState<number>(5);

  // Form State for Funding Carry Strategy
  const [fundingTradeSizeUsd, setFundingTradeSizeUsd] = useState<number>(25000);
  const [fundingMinAprPct, setFundingMinAprPct] = useState<number>(8.0);
  const [fundingHoldingDays, setFundingHoldingDays] = useState<number>(14);
  const [fundingMaxSlippageBps, setFundingMaxSlippageBps] = useState<number>(15.0);
  const [fundingMaxEntrySpreadBps, setFundingMaxEntrySpreadBps] = useState<number>(50.0);
  const [fundingAutoExitSpreadPct, setFundingAutoExitSpreadPct] = useState<string>("");
  const [fundingRotationThresholdBps, setFundingRotationThresholdBps] = useState<string>("");
  const [fundingAutoRebalance, setFundingAutoRebalance] = useState<boolean>(false);

  // Universal trim block (one threshold per strategy, "" = off; identical
  // trim-both / close-both semantics everywhere via the shared helper).
  const [futuresLiqTrimPct, setFuturesLiqTrimPct] = useState<string>("");
  const [fundingLiqTrimPct, setFundingLiqTrimPct] = useState<string>("");
  const [spotLiqTrimPct, setSpotLiqTrimPct] = useState<string>("");

  // Per-strategy slippage for the futures/spread path.
  const [futuresMaxSlippageBps, setFuturesMaxSlippageBps] = useState<number>(15.0);

  // Passive limit unwinds/rebalances (paper-only): one control set per
  // strategy with a live reduce path. Off by default (opt-in).
  const [futuresPassive, setFuturesPassive] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });
  const [fundingPassive, setFundingPassive] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });
  const [spotPassive, setSpotPassive] = useState<PassiveUnwindDto>({ ...DEFAULT_PASSIVE_UNWIND });

  // Form State for Spot Flash Strategy
  const [spotTradeSizeUsd, setSpotTradeSizeUsd] = useState<number>(1000);
  const [spotMaxSlippageBps, setSpotMaxSlippageBps] = useState<number>(5.0);
  const [spotMinNetSpreadBps, setSpotMinNetSpreadBps] = useState<number>(15.0);

  // Per-strategy auto-hide filters ("" = off). Shared by futures/funding/spot;
  // each strategy record carries its own thresholds.
  const [autoMinVolumeUsd, setAutoMinVolumeUsd] = useState<string>("");
  const [autoMinPriceUsd, setAutoMinPriceUsd] = useState<string>("");
  // Prediction event-volume threshold has no venue data yet, so it is
  // display-only (PredictionHub) and echoed back untouched here.
  const [autoMinEventVolumeUsd, setAutoMinEventVolumeUsd] = useState<number | null>(null);
  // Manual hide list (global, shared by all strategies; applies AFTER auto).
  const [exemptList, setExemptList] = useState<string[] | null>(null);
  const [revealBusy, setRevealBusy] = useState(false);

  // UI status
  const [isSaving, setIsSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

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
        setExecutionMode((p.executionMode || "SINGLE_WAY") as ArbitrageExecutionMode);
        setStepSizeMult(Number(p.stepSizeMult ?? 10));
        setMaxStepUsd(Number(p.maxStepUsd ?? 2500));
        setMaxTickerEquityPct(Number(p.maxTickerEquityPct ?? 20));
        setUserTargetLeverage(Number(p.userTargetLeverage ?? 5));
        setFundingTradeSizeUsd(Number(p.fundingTradeSizeUsd ?? 25000));
        setFundingMinAprPct(Number(p.fundingMinAprPct ?? 8.0));
        setFundingHoldingDays(Number(p.fundingHoldingDays ?? 14));
        setFundingMaxSlippageBps(Number(p.fundingMaxSlippageBps ?? 15.0));
        setFundingMaxEntrySpreadBps(Number(p.fundingMaxEntrySpreadBps ?? 50.0));
        setFundingAutoExitSpreadPct(p.fundingAutoExitSpreadPct != null ? String(p.fundingAutoExitSpreadPct) : "");
        setFundingRotationThresholdBps(p.fundingRotationThresholdBps != null ? String(p.fundingRotationThresholdBps) : "");
        setFundingAutoRebalance(p.fundingAutoRebalance === true);
        setFundingLiqTrimPct(p.fundingLiqTrimPct != null ? String(p.fundingLiqTrimPct) : "");
        setFuturesMaxSlippageBps(Number(p.futuresMaxSlippageBps ?? 15.0));
        setFuturesLiqTrimPct(p.futuresLiqTrimPct != null ? String(p.futuresLiqTrimPct) : "");
        setSpotLiqTrimPct(p.spotLiqTrimPct != null ? String(p.spotLiqTrimPct) : "");
        setSpotTradeSizeUsd(Number(p.spotTradeSizeUsd ?? 1000));
        setSpotMaxSlippageBps(Number(p.spotMaxSlippageBps ?? 5.0));
        setSpotMinNetSpreadBps(Number(p.spotMinNetSpreadBps ?? 15.0));
        setAutoMinVolumeUsd(p.autoMinVolumeUsd != null ? String(p.autoMinVolumeUsd) : "");
        setAutoMinPriceUsd(p.autoMinPriceUsd != null ? String(p.autoMinPriceUsd) : "");
        setAutoMinEventVolumeUsd(p.autoMinEventVolumeUsd ?? null);
        setFuturesPassive({ ...DEFAULT_PASSIVE_UNWIND, ...(p.futuresPassiveUnwind ?? {}) });
        setFundingPassive({ ...DEFAULT_PASSIVE_UNWIND, ...(p.fundingPassiveUnwind ?? {}) });
        setSpotPassive({ ...DEFAULT_PASSIVE_UNWIND, ...(p.spotPassiveUnwind ?? {}) });
      })
      .catch((err) => {
        console.warn("Failed to load strategy params in modal:", err);
      });

    // Manual hide list is global and shared — load it whenever the modal opens.
    api
      .getExemptedTickers()
      .then((l) => setExemptList(l))
      .catch(() => {
        setExemptList(null);
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

    // Total paper equity backing the derived max-steps preview; unknown on
    // balances failure so the preview sizes to 0 steps instead of a seed.
    api
      .getVenueBalances()
      .then((balances) => {
        setTotalEquityUsd(resolveTotalPaperEquity(balances ?? []));
      })
      .catch(() => {
        setTotalEquityUsd(null);
      });
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

  // Universal sizing is authored per strategy; the context ticker's step
  // quantity and auto max steps derive from the canonical helper — the same
  // helper the matrix and scan paths use, so preview/execute cannot diverge.
  const sizing = useMemo(() => {
    return deriveStepSizing(ticker.symbol, ticker.venues, ticker.basePrice, totalEquityUsd, {
      stepSizeMult,
      maxStepUsd,
      maxTickerEquityPct,
    });
  }, [ticker, stepSizeMult, maxStepUsd, maxTickerEquityPct, totalEquityUsd]);
  const unitSize = sizing.stepBaseQty;
  const maxAccumulatedUnits = sizing.maxSteps;

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
      ? "Auto-tune needs 24h+ of spread history"
      : baselineSnapshot.qualifying_pair_count === 0
        ? "No qualifying venue pairs — auto-tune unavailable"
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
    setFundingTradeSizeUsd(25000);
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
    setFuturesPassive({ ...DEFAULT_PASSIVE_UNWIND });
    setFundingPassive({ ...DEFAULT_PASSIVE_UNWIND });
    setSpotPassive({ ...DEFAULT_PASSIVE_UNWIND });
  };

  const handleRevealAll = async () => {
    setRevealBusy(true);
    try {
      await api.revealAllTickers();
      setExemptList([]);
    } catch {
      /* leave the list as-is on failure */
    } finally {
      setRevealBusy(false);
    }
  };

  const handleUnexempt = async (base: string) => {
    try {
      await api.setTickerExempted(base, false);
      setExemptList((l) => (l == null ? l : l.filter((b) => b !== base)));
    } catch {
      /* leave the list as-is on failure */
    }
  };

  const parseAutoThreshold = (raw: string): number | null => {
    const v = parseFloat(raw);
    return Number.isFinite(v) && v > 0 ? v : null;
  };

  // Shared trim block: every bot configurator surfaces the same threshold
  // input with identical trim-both/close-both semantics (one backend
  // helper, per-strategy persistence). "" = off.
  const renderTrimBlock = (value: string, setValue: (v: string) => void) => (
    <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
      <label className="text-slate-700 font-medium flex-1">
        <span className="flex items-center">
          Liquidation-Distance Trim (%)
          <InfoTooltip text="When EITHER leg comes within this percent of liquidation, BOTH legs trim by the same qty to stay neutral; if the remainder would drop below one step, both close instead. Empty = off." />
        </span>
        <span className="block text-xs text-slate-400 font-normal mt-0.5">Shared trim block · trim-both, else close-both · empty = off</span>
      </label>
      <input
        type="number"
        step="0.5"
        min="0"
        max="100"
        value={value}
        placeholder="Off"
        onChange={(e) => setValue(e.target.value)}
        className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
      />
    </div>
  );

  // Shared passive-unwind block: reduce-only limits pegged inside the
  // spread with a timeout-bound retry loop and market fallback. One
  // instance per strategy tab; the calm gate skips passive when the live
  // spread or fast vol exceeds its cap.
  const renderPassiveBlock = (value: PassiveUnwindDto, setValue: (v: PassiveUnwindDto) => void) => {
    const set = (patch: Partial<PassiveUnwindDto>) => setValue({ ...value, ...patch });
    const numInput = (
      label: string,
      hint: string,
      current: number,
      apply: (n: number) => void,
      step: string,
      min: string,
      max?: string,
    ) => (
      <div className="flex items-center justify-between gap-3 py-1">
        <label className="text-slate-600 font-medium flex-1">
          <span>{label}</span>
          <span className="block text-xs text-slate-400 font-normal mt-0.5">{hint}</span>
        </label>
        <input
          type="number"
          step={step}
          min={min}
          max={max}
          value={current}
          onChange={(e) => {
            const n = parseFloat(e.target.value);
            if (Number.isFinite(n)) apply(n);
          }}
          disabled={!value.enabled}
          className="w-24 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-40"
        />
      </div>
    );
    return (
      <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs space-y-1">
        <div className="flex items-center justify-between gap-3">
          <label className="text-slate-700 font-medium flex-1">
            <span className="flex items-center">
              Passive Limit Unwind
              <InfoTooltip text="Close reduces with reduce-only LIMITs pegged inside the spread (0 = aggressive touch, 50 = mid, 100 = passive edge). Unfilled attempts wait timeoutSecs, re-price to a fresh book, and retry up to maxTries, then fall back to market. Skipped when spread or fast vol exceeds its cap. Off = market exits as today." />
            </span>
            <span className="block text-xs text-slate-400 font-normal mt-0.5">Paper-only · timeout-bound · market fallback · off = current behavior</span>
          </label>
          <SwitchToggle
            size="sm"
            label={value.enabled ? "On" : "Off"}
            checked={value.enabled}
            onChange={(on) => set({ enabled: on })}
          />
        </div>
        {numInput("Spread percentile", "0 aggressive · 50 mid · 100 passive edge", value.spreadPercentile, (n) => set({ spreadPercentile: Math.max(0, Math.min(100, n)) }), "5", "0", "100")}
        {numInput("Limit timeout (s)", "Wait per unfilled try before re-pricing", value.timeoutSecs, (n) => set({ timeoutSecs: Math.max(0, Math.min(300, Math.round(n))) }), "1", "0", "300")}
        {numInput("Max limit tries", "Market fallback after this many tries", value.maxTries, (n) => set({ maxTries: Math.max(1, Math.min(10, Math.round(n))) }), "1", "1", "10")}
        {numInput("Max spread (BPS)", "Skip passive above this live spread", value.maxSpreadBps, (n) => set({ maxSpreadBps: Math.max(0.1, n) }), "1", "0.1")}
        {numInput("Max fast vol (BPS)", "Skip passive above this fast vol", value.maxFastVolBps, (n) => set({ maxFastVolBps: Math.max(0.1, n) }), "1", "0.1")}
      </div>
    );
  };

  const handleSave = async () => {
    setIsSaving(true);
    setSaveSuccess(false);
    setSaveError(null);

    const canonicalWindow = formatDuration(manualWindowSecs);
    // Prediction has no configurator surface: re-read the record and echo
    // its inert values untouched (schema parity, UI-disabled like
    // autoMinEventVolumeUsd). A failed re-read falls back to schema
    // defaults rather than fabricating values.
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
      futuresLiqTrimPct: parseOptThreshold(futuresLiqTrimPct),
      spotLiqTrimPct: parseOptThreshold(spotLiqTrimPct),
      predictionLiqTrimPct: currentRecord?.predictionLiqTrimPct ?? null,
      spotTradeSizeUsd,
      spotMaxSlippageBps,
      spotMinNetSpreadBps,
      futuresPassiveUnwind:
        activeStrategyTab === "futures" ? futuresPassive : (currentRecord?.futuresPassiveUnwind ?? { ...DEFAULT_PASSIVE_UNWIND }),
      fundingPassiveUnwind:
        activeStrategyTab === "funding" ? fundingPassive : (currentRecord?.fundingPassiveUnwind ?? { ...DEFAULT_PASSIVE_UNWIND }),
      spotPassiveUnwind:
        activeStrategyTab === "spot" ? spotPassive : (currentRecord?.spotPassiveUnwind ?? { ...DEFAULT_PASSIVE_UNWIND }),
    };

    const validationErrors = validateStrategyParams(updatedParams);
    if (validationErrors.length > 0) {
      setSaveError(validationErrors[0]);
      setIsSaving(false);
      return;
    }

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
      setSaveError(
        err instanceof Error ? err.message : "Save failed — parameters were not written."
      );
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
                      className={`w-full flex items-center space-x-2.5 px-3 py-2.5 rounded-lg font-mono text-[11px] transition cursor-pointer text-left ${
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
          <div className="flex-1 overflow-y-auto p-4 space-y-4 bg-surface">
            {/* TAB 1: Futures Spread Convergence */}
            {activeStrategyTab === "futures" && (
              <div className="space-y-5">
              {activeSection === "hurdle" && (
              <div className="space-y-3">
                <div className="flex items-center justify-between">
                  <span className="text-xs font-bold text-slate-700">Signal Trigger</span>
                  <button
                    type="button"
                    onClick={handleOpenAutotune}
                    disabled={autotuneDisabledReason != null}
                    title={autotuneDisabledReason ?? "Walk-forward recommend-only preview (70/30 split, validated per parameter)"}
                    className="px-3 py-1.5 rounded-md font-mono text-[11px] font-semibold border transition cursor-pointer disabled:cursor-not-allowed bg-accent-dim border-accent-line text-accent hover:bg-blue-100 disabled:bg-surface-raised disabled:border-surface-border disabled:text-slate-400"
                  >
                    Auto-tune
                  </button>
                </div>
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-xs text-slate-700 font-medium flex items-center flex-1">
                    <span className="flex items-center">
                      Target Profit (BPS)
                      <InfoTooltip text="Net profit cushion locked in per round-trip after deducting taker fees and closing spread." />
                    </span>
                  </label>
                  <div className="flex items-center space-x-2 shrink-0">
                    <input
                      type="number"
                      step="0.5"
                      min="0.1"
                      max="100"
                      value={targetProfitBps}
                      onChange={(e) => setTargetProfitBps(Math.max(0.1, parseFloat(e.target.value) || 0.1))}
                      className="w-28 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
                    />
                    <span className="text-xs text-slate-500 font-semibold">bps</span>
                  </div>
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-xs text-slate-700 font-medium flex items-center flex-1">
                    <span className="flex items-center">
                      Max Slippage (BPS)
                      <InfoTooltip text="Per-strategy slippage gate: entries whose VWAP slips beyond this tolerance are refused. Replaces the former global setting." />
                    </span>
                  </label>
                  <div className="flex items-center space-x-2 shrink-0">
                    <input
                      type="number"
                      step="0.5"
                      min="0.1"
                      value={futuresMaxSlippageBps}
                      onChange={(e) => setFuturesMaxSlippageBps(Math.max(0.1, parseFloat(e.target.value) || 15))}
                      className="w-28 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
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

                <SessionProfileCard symbol={tickerSymbol} />

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-xs text-slate-700 font-medium flex items-center flex-1">
                    <span className="flex items-center">
                      Window Percentile
                      <InfoTooltip text="Strictly filters entries to spreads at or above this percentile of the rolling sliding window." />
                    </span>
                    <span className="text-xs text-slate-400 ml-2">Min 50%</span>
                  </label>                  <select
                    value={windowPercentile}
                    onChange={(e) => setWindowPercentile(Math.max(50, parseInt(e.target.value, 10)))}
                    className="w-44 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-medium text-xs focus:outline-none focus:ring-1 focus:ring-accent h-[38px]"
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
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-slate-700 font-medium flex items-center justify-between gap-2 flex-1">
                    <span>Step Size Multiplier (× min lot)</span>
                    <span className="text-xs text-slate-400">≥ 1</span>
                  </label>
                  <input
                    type="number"
                    step={1}
                    min={1}
                    value={stepSizeMult}
                    onChange={(e) =>
                      setStepSizeMult(Math.max(1, parseFloat(e.target.value) || 10))
                    }
                    className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-slate-700 font-medium flex items-center justify-between gap-2 flex-1">
                    <span>Max Step (USD)</span>
                    <span className="text-xs text-slate-400">Notional cap / step</span>
                  </label>
                  <input
                    type="number"
                    step={100}
                    min={10}
                    value={maxStepUsd}
                    onChange={(e) =>
                      setMaxStepUsd(Math.max(10, parseFloat(e.target.value) || 2500))
                    }
                    className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-slate-700 font-medium flex items-center justify-between gap-2 flex-1">
                    <span>Max Ticker Equity (%)</span>
                    <span className="text-xs text-slate-400">0 – 100</span>
                  </label>
                  <input
                    type="number"
                    step={0.5}
                    min={0.1}
                    max={100}
                    value={maxTickerEquityPct}
                    onChange={(e) =>
                      setMaxTickerEquityPct(Math.max(0.1, Math.min(100, parseFloat(e.target.value) || 20)))
                    }
                    className="w-28 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
              </div>

              )}

              {activeSection === "leverage" && (
              <div className="space-y-3 text-xs">
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-slate-700 font-medium flex-1">Target Leverage</label>
                  <div className="flex items-center space-x-1.5 shrink-0">
                    <input
                      type="number"
                      step="1"
                      min="1"
                      max="5"
                      value={userTargetLeverage}
                      onChange={(e) =>
                        setUserTargetLeverage(Math.max(1, Math.min(5, parseInt(e.target.value, 10) || 5)))
                      }
                      className="w-24 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
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
                  <span className="text-xs text-slate-500 font-semibold">Liquidation Buffer</span>
                  <div className="text-sm font-bold text-emerald-700">
                    {safeLeverage.liquidationBufferPct.toFixed(1)}%
                  </div>
                  <span className="text-xs text-slate-400">Dual-leg safety margin</span>
                </div>

                {renderTrimBlock(futuresLiqTrimPct, setFuturesLiqTrimPct)}

                {renderPassiveBlock(futuresPassive, setFuturesPassive)}
              </div>
              )}
              {activeSection === "visibility" && (
              <div className="space-y-3 text-xs">
                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-slate-700 font-medium flex-1">
                    <span className="flex items-center">
                      Auto Min Volume (USD, 24h)
                      <InfoTooltip text="Hide tickers whose MINIMUM 24h volume across in-scope venues falls below this floor. Any venue missing volume data excludes the ticker (fail-closed). Empty = off." />
                    </span>
                    <span className="block text-xs text-slate-400 font-normal mt-0.5">Automatic · MIN across venues defeats single-venue wash inflation</span>
                  </label>
                  <input
                    type="number"
                    step="1000"
                    min="0"
                    value={autoMinVolumeUsd}
                    placeholder="Off"
                    onChange={(e) => setAutoMinVolumeUsd(e.target.value)}
                    className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                  <label className="text-slate-700 font-medium flex-1">
                    <span className="flex items-center">
                      Auto Min Price (USD)
                      <InfoTooltip text="Hide tickers whose minimum mid price across in-scope venues falls below this floor. Any venue missing a mid excludes the ticker (fail-closed). Empty = off." />
                    </span>
                    <span className="block text-xs text-slate-400 font-normal mt-0.5">Automatic · uses the minimum mid across in-scope venues</span>
                  </label>
                  <input
                    type="number"
                    step="0.01"
                    min="0"
                    value={autoMinPriceUsd}
                    placeholder="Off"
                    onChange={(e) => setAutoMinPriceUsd(e.target.value)}
                    className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>

                <div className="bg-surface p-3.5 rounded-lg border border-surface-border space-y-3 shadow-2xs">
                  <div className="flex items-center justify-between gap-4">
                    <div className="space-y-1 min-w-0">
                      <label className="text-slate-800 font-bold block text-xs">Hidden Tickers (manual)</label>
                      <p className="text-xs text-slate-500">
                        {exemptList === null
                          ? "Hidden tickers are excluded from the Opportunity Radar and strategy selection."
                          : exemptList.length === 0
                            ? "No tickers are currently hidden."
                            : `${exemptList.length} ticker${exemptList.length === 1 ? "" : "s"} hidden.`}
                      </p>
                      <p className="text-xs text-slate-400">Manual list · shared by all strategies · applies after auto-filtering</p>
                    </div>
                    <button
                      type="button"
                      disabled={revealBusy || !exemptList?.length}
                      onClick={() => void handleRevealAll()}
                      className="px-3 py-1.5 rounded-md border border-surface-border text-xs font-mono font-semibold text-slate-500 hover:text-accent hover:border-accent/40 transition cursor-pointer disabled:opacity-40 shrink-0"
                    >
                      {revealBusy ? "…" : "Reveal All"}
                    </button>
                  </div>
                  {exemptList && exemptList.length > 0 && (
                    <div className="flex flex-wrap gap-1.5">
                      {exemptList.map((base) => (
                        <button
                          key={base}
                          type="button"
                          onClick={() => void handleUnexempt(base)}
                          title={`Un-hide ${base}`}
                          className="flex items-center gap-1 px-2 py-1 rounded-sm border border-surface-border bg-surface-raised text-xs font-mono font-bold text-slate-600 hover:text-accent hover:border-accent/40 transition cursor-pointer"
                        >
                          {base}
                          <span className="text-slate-400">×</span>
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              </div>
              )}
            </div>
          )}

          {/* TAB 2: Funding Carry Radar */}
          {activeStrategyTab === "funding" && activeSection === "carry" && (
            <div className="space-y-3 text-xs">
              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Trade Size (USD)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Notional Per Leg</span>
                </label>
                <input
                  type="number"
                  step="1000"
                  min="1000"
                  value={fundingTradeSizeUsd}
                  onChange={(e) => setFundingTradeSizeUsd(Math.max(100, parseFloat(e.target.value) || 1000))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Min APR Hurdle (%)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Ignore opportunities below this net APR</span>
                </label>
                <input
                  type="number"
                  step="0.5"
                  min="1"
                  value={fundingMinAprPct}
                  onChange={(e) => setFundingMinAprPct(Math.max(0.1, parseFloat(e.target.value) || 1))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Holding Horizon (Days)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Projected Carry Amortization Duration</span>
                </label>
                <input
                  type="number"
                  step="1"
                  min="1"
                  max="365"
                  value={fundingHoldingDays}
                  onChange={(e) => setFundingHoldingDays(Math.max(1, parseInt(e.target.value, 10) || 14))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Max Slippage (BPS)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Per-strategy slippage gate for funding entries</span>
                </label>
                <input
                  type="number"
                  step="0.5"
                  min="0.1"
                  value={fundingMaxSlippageBps}
                  onChange={(e) => setFundingMaxSlippageBps(Math.max(0.1, parseFloat(e.target.value) || 15))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Max Entry Spread (BPS)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Refuse entry when the venue execution-price gap exceeds this</span>
                </label>
                <input
                  type="number"
                  step="1"
                  min="0.1"
                  value={fundingMaxEntrySpreadBps}
                  onChange={(e) => setFundingMaxEntrySpreadBps(Math.max(0.1, parseFloat(e.target.value) || 50))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Auto-Exit Spread Threshold (%)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Exit when inter-venue spread drops below this % · empty = off</span>
                </label>
                <input
                  type="number"
                  step="0.5"
                  min="0"
                  max="100"
                  value={fundingAutoExitSpreadPct}
                  placeholder="Off"
                  onChange={(e) => setFundingAutoExitSpreadPct(e.target.value)}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              {renderTrimBlock(fundingLiqTrimPct, setFundingLiqTrimPct)}

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span className="flex items-center">
                    Auto-Rebalance to Neutral
                    <InfoTooltip text="Reduce the overweight leg back to neutral when the pair drifts." />
                  </span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Trims the overweight leg like the spread bot</span>
                </label>
                <SwitchToggle
                  size="sm"
                  label={fundingAutoRebalance ? "On" : "Off"}
                  checked={fundingAutoRebalance}
                  onChange={setFundingAutoRebalance}
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Rotation Threshold (BPS)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Rotate only when net-of-friction improvement exceeds this · empty = off</span>
                </label>
                <input
                  type="number"
                  step="1"
                  min="0"
                  value={fundingRotationThresholdBps}
                  placeholder="Off"
                  onChange={(e) => setFundingRotationThresholdBps(e.target.value)}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              {renderPassiveBlock(fundingPassive, setFundingPassive)}

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Auto Min Volume (USD, 24h)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Automatic · MIN across in-scope venues · empty = off</span>
                </label>
                <input
                  type="number"
                  step="1000"
                  min="0"
                  value={autoMinVolumeUsd}
                  placeholder="Off"
                  onChange={(e) => setAutoMinVolumeUsd(e.target.value)}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Auto Min Price (USD)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Automatic · minimum mid across in-scope venues · empty = off</span>
                </label>
                <input
                  type="number"
                  step="0.01"
                  min="0"
                  value={autoMinPriceUsd}
                  placeholder="Off"
                  onChange={(e) => setAutoMinPriceUsd(e.target.value)}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>
            </div>
          )}

          {/* TAB 3: Spot Arbitrage */}
          {activeStrategyTab === "spot" && activeSection === "spatial" && (
            <div className="space-y-3 text-xs">
              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Trade Size (USD)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Capital deployed per flash cycle</span>
                </label>
                <input
                  type="number"
                  step="100"
                  min="100"
                  value={spotTradeSizeUsd}
                  onChange={(e) => setSpotTradeSizeUsd(Math.max(10, parseFloat(e.target.value) || 500))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Max Slippage (BPS)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Orderbook Tolerance Limit</span>
                </label>
                <input
                  type="number"
                  step="0.5"
                  min="0.5"
                  max="50"
                  value={spotMaxSlippageBps}
                  onChange={(e) => setSpotMaxSlippageBps(Math.max(0.1, parseFloat(e.target.value) || 5))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Min Net Spread (BPS)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Net spread required to auto-execute</span>
                </label>
                <input
                  type="number"
                  step="0.5"
                  min="0.1"
                  value={spotMinNetSpreadBps}
                  onChange={(e) => setSpotMinNetSpreadBps(Math.max(0.1, parseFloat(e.target.value) || 1))}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              {renderTrimBlock(spotLiqTrimPct, setSpotLiqTrimPct)}

              {renderPassiveBlock(spotPassive, setSpotPassive)}

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Auto Min Volume (USD, 24h)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Automatic · MIN across in-scope venues · empty = off</span>
                </label>
                <input
                  type="number"
                  step="1000"
                  min="0"
                  value={autoMinVolumeUsd}
                  placeholder="Off"
                  onChange={(e) => setAutoMinVolumeUsd(e.target.value)}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              <div className="bg-surface p-3.5 rounded-lg border border-surface-border shadow-2xs flex items-center justify-between gap-3">
                <label className="text-slate-700 font-medium flex-1">
                  <span>Auto Min Price (USD)</span>
                  <span className="block text-xs text-slate-400 font-normal mt-0.5">Automatic · minimum mid across in-scope venues · empty = off</span>
                </label>
                <input
                  type="number"
                  step="0.01"
                  min="0"
                  value={autoMinPriceUsd}
                  placeholder="Off"
                  onChange={(e) => setAutoMinPriceUsd(e.target.value)}
                  className="w-28 shrink-0 px-3 py-2 bg-surface-raised border border-surface-border rounded-md text-slate-900 font-bold focus:outline-none focus:ring-1 focus:ring-accent"
                />
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
            {saveError && (
              <span role="alert" className="text-xs font-semibold text-red-600 max-w-60 text-right">
                {saveError}
              </span>
            )}
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
