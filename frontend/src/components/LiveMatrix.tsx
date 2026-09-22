import React, { useState, useEffect, useMemo, useCallback } from "react";
import {
  TrendingUp,
  TrendingDown,
  Zap,
  History,
  Terminal,
  SlidersHorizontal,
  LayoutGrid,
  Radar,
  Table,
  X,
} from "lucide-react";
import type {
  VenueOrderBookDto,
  SpatialOpportunityDto,
  ExecutionLogEntry,
  MatrixBaselineSnapshotDto,
  MatrixCellDto,
  TickerStrategyConfigDto,
  StrategyParamsDto,
  TickerPositionState,
} from "../types";
import {
  PERP_TICKERS,
  formatCurrencyPrice,
  formatCurrencyPnl,
  getDefaultOrderQty,
  getTickerActiveVenues,
  type PerpTicker,
} from "../types/tickers";
import {
  buildCrossVenueMatrix,
  formatDuration,
  getMatrixCellVisuals,
  globalSpreadTracker,
  parseDuration,
} from "../services/matrixQuant";
import { deriveStepSizing } from "../services/contractSpecs";
import { isPositionReduceOnly } from "../services/positionsQuant";
import { formatWinPct as fmtPct } from "../services/winPct";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { useBotLifecycle } from "../hooks/useBotLifecycle";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerTelemetryGadget } from "./TickerTelemetryGadget";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { TickerOpportunityHistory } from "./TickerOpportunityHistory";
import { ExecuteButton } from "./ExecuteButton";
import { CapacityChart } from "./CapacityChart";
import { DetailField, DetailSection } from "./DetailBits";
import { SignalTrendGadget } from "./SignalTrendGadget";
import { TickerConfigModal } from "./TickerConfigModal";
import { EmptyState } from "./EmptyState";
import { VenueLogo } from "./VenueLogo";
import { api } from "../services/api";

interface LiveMatrixProps {
  /** Selected ticker; null until the user explicitly picks one in the explorer. */
  symbol: string | null;
  orderbooks: VenueOrderBookDto[];
  opportunities: SpatialOpportunityDto[];
  onRefresh: (qty?: number, minBps?: number) => void;
  onExecuteTrade: (opp: SpatialOpportunityDto) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenTickerConfig?: (symbol: string, strategy?: string) => void;
  activeVenueCodes?: Set<string>;
  availableTickers?: PerpTicker[];
  isTickerEnabled?: boolean;
  onToggleTicker?: (symbol: string, enabled?: boolean) => void;
  isPaused?: boolean;
  positionState?: TickerPositionState | null;
  futuresParams?: StrategyParamsDto | null;
  /** Summed paper-equity backing the derived per-ticker max steps; null = unknown, sizes to 0 steps. */
  totalEquityUsd?: number | null;
  /** Left-column ticker selector gadget, composed by the parent — symbol
   *  selection is strategy-specific so it lives on this page, not the
   *  universal sidebar. */
  tickerExplorer?: React.ReactNode;
}

interface MatrixCellViewProps {
  cell: MatrixCellDto;
  isSelected: boolean;
  minBps: number;
  onSelect: (rowId: string, colId: string) => void;
}

const MatrixCellView: React.FC<MatrixCellViewProps> = React.memo(({ cell, isSelected, minBps, onSelect }) => {
  const visuals = getMatrixCellVisuals(cell, minBps);
  if (cell.isDiagonal) {
    return (
      <td className="h-10 overflow-hidden whitespace-nowrap text-center border-r border-surface-border last:border-r-0 bg-surface-raised/40 select-none">
        <span className="block truncate text-slate-400 font-mono text-[10px]">—</span>
      </td>
    );
  }
  return (
    <td
      onClick={() => onSelect(cell.rowId, cell.colId)}
      className={`h-10 overflow-hidden whitespace-nowrap p-1 text-center border-r border-surface-border last:border-r-0 cursor-pointer transition select-none ${
        visuals.bgClass
      } ${isSelected ? "ring-2 ring-accent shadow-2xs" : ""}`}
      title={
        cell.costReady
          ? `Commission ${cell.roundTripCommissionBps?.toFixed(1)} BPS · entry slippage ${cell.entrySlippageBps?.toFixed(1)} BPS · exit slippage ${cell.exitSlippageBps?.toFixed(1)} BPS · exit crossing ${cell.closingFrictionBps.toFixed(1)} BPS · total friction ${cell.expectedFrictionBps?.toFixed(1)} BPS · required gross ${cell.requiredGrossBps?.toFixed(1)} BPS`
          : "Route economics unavailable"
      }
    >
      <div className="flex flex-col items-center justify-center space-y-0.5 overflow-hidden">
        <div className={`text-[10px] font-mono font-bold whitespace-nowrap truncate max-w-full ${visuals.textClass}`}>
          {cell.costReady === false ? (
            "—"
          ) : (
            <>
              {cell.netSpreadBps > 0 ? "+" : ""}
              {cell.netSpreadBps.toFixed(1)} BPS
            </>
          )}
        </div>
        <div
          className={`flex items-center space-x-0.5 px-1 py-0.2 rounded-lg text-[10px] font-mono whitespace-nowrap truncate max-w-full ${visuals.deltaBadgeBg}`}
        >
          {cell.baselineReady === false || cell.sampleCount === 0 ? (
            <span>Δ —</span>
          ) : (
            <>
              {cell.deltaBps > 0 ? (
                <TrendingUp className="w-2 h-2 inline" />
              ) : cell.deltaBps < 0 ? (
                <TrendingDown className="w-2 h-2 inline" />
              ) : null}
              <span>
                Δ {cell.deltaBps > 0 ? "+" : ""}
                {cell.deltaBps.toFixed(1)}
              </span>
            </>
          )}
        </div>
      </div>
    </td>
  );
});

interface OpportunityRowViewProps {
  opp: SpatialOpportunityDto;
  minBps: number;
  blocked: boolean;
  blockingIds: string;
  botPhase: string;
  loading: boolean;
  symbolLabel: string | null;
  onExecute: (opp: SpatialOpportunityDto) => void;
  onSelect: (opp: SpatialOpportunityDto) => void;
}

const LIVE_CAPABLE_VENUES = new Set([
  "aster",
  "hyperliquid",
  "lighter",
  "robinhood",
  "vanta",
  "binance",
  "okx",
  "coinbase",
  "kraken",
]);
const isLiveRoute = (v1: string, v2: string): boolean =>
  LIVE_CAPABLE_VENUES.has(v1.toLowerCase()) && LIVE_CAPABLE_VENUES.has(v2.toLowerCase());

const OpportunityRowView: React.FC<OpportunityRowViewProps> = React.memo(
  ({ opp, minBps, blocked, blockingIds, botPhase, loading, symbolLabel, onExecute, onSelect }) => {
    const isExec = opp.is_executable;
    const entriesOpen = botPhase === "idle" || botPhase === "running";
    const disabled = !isExec || loading || blocked || !entriesOpen;
    const title = blocked
      ? `${symbolLabel} blocked: signal ${blockingIds} awaiting delta-neutral reconciliation`
      : botPhase === "paused" || botPhase === "reduce_only" || botPhase === "stopping"
        ? `Entries blocked: bot is ${botPhase.replace("_", "-")}`
        : undefined;
    const isLive = isLiveRoute(opp.buy_venue, opp.sell_venue);
    return (
      <tr
        className={`hover:bg-surface-raised transition cursor-pointer ${isExec ? "bg-accent-dim/30" : ""}`}
        onClick={() => onSelect(opp)}
        title="Click for opportunity details"
      >
        <td className="py-2.5 px-3 font-medium text-slate-900 flex items-center space-x-1.5">
          <span className="flex items-center gap-1 min-w-0">
            <VenueLogo venueId={(opp.buy_venue_id ?? opp.buy_venue ?? "").toLowerCase()} name={opp.buy_venue} size={16} />
            <span className="text-slate-800 font-semibold">{opp.buy_venue}</span>
          </span>
          <span className="text-slate-400 text-xs">➔</span>
          <span className="flex items-center gap-1 min-w-0">
            <VenueLogo venueId={(opp.sell_venue_id ?? opp.sell_venue ?? "").toLowerCase()} name={opp.sell_venue} size={16} />
            <span className="text-slate-800 font-semibold">{opp.sell_venue}</span>
          </span>
          {!isLive && (
            <span
              className="ml-1.5 px-1.5 py-0.5 rounded-md border border-surface-border bg-surface-raised text-slate-500 text-[10px] font-mono"
              title="Route contains read-only venue(s) — paper execution only"
            >
              PAPER
            </span>
          )}
        </td>
        <td className="py-2.5 px-3 text-loss font-medium font-mono tabular-nums">{formatCurrencyPrice(opp.buy_price)}</td>
        <td className="py-2.5 px-3 text-profit font-medium font-mono tabular-nums">{formatCurrencyPrice(opp.sell_price)}</td>
        <td className="py-2.5 px-3 text-slate-700 font-mono tabular-nums">{opp.gross_spread_bps.toFixed(1)} BPS</td>
        <td className="py-2.5 px-3 text-slate-500 font-mono tabular-nums">{opp.fee_bps.toFixed(1)} BPS</td>
        <td className="py-2.5 px-3 font-mono tabular-nums">
          <span className={`font-bold ${opp.net_spread_bps >= minBps ? "text-profit" : "text-slate-500"}`}>
            {opp.net_spread_bps.toFixed(2)} BPS
          </span>
        </td>
        <td className="py-2.5 px-3 text-slate-700 font-mono tabular-nums">{fmtPct(opp.win_probability)}</td>
        <td className={`py-2.5 px-3 font-bold font-mono tabular-nums ${opp.estimated_profit_usd >= 0 ? "text-profit" : "text-loss"}`}>
          {formatCurrencyPnl(opp.estimated_profit_usd)}
        </td>
        <td className="py-2.5 px-3 text-right" onClick={(e) => e.stopPropagation()}>
          <ExecuteButton
            onClick={() => onExecute(opp)}
            disabled={disabled}
            title={title}
            className="px-2 py-2 text-xs shadow-2xs"
          />
        </td>
      </tr>
    );
  },
);

interface SpatialDetailModalProps {
  opp: SpatialOpportunityDto;
  executing: boolean;
  blocked: boolean;
  onExecute: (opp: SpatialOpportunityDto) => void;
  onClose: () => void;
}

/** Click-through detail for one spatial route: leg prices, friction
 *  breakdown, win evidence, and the capacity curve over both leg books.
 *  Read-only over the scanned DTO — never synthesizes. */
const SpatialDetailModal: React.FC<SpatialDetailModalProps> = React.memo(
  ({ opp, executing, blocked, onExecute, onClose }) => {
    useEffect(() => {
      const handleKeyDown = (e: KeyboardEvent) => {
        if (e.key === "Escape") onClose();
      };
      window.addEventListener("keydown", handleKeyDown);
      return () => window.removeEventListener("keydown", handleKeyDown);
    }, [onClose]);
    const fmtUsd = (v: number | null | undefined): string =>
      v != null && Number.isFinite(v)
        ? `$${v.toLocaleString(undefined, { maximumFractionDigits: 0 })}`
        : "—";
    const gate = (ok: boolean) => (
      <span className={`font-bold ${ok ? "text-profit" : "text-loss"}`}>{ok ? "PASS" : "FAIL"}</span>
    );
    return (
      <div
        className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
      >
        <div className="bg-surface border border-surface-border rounded-2xl w-[800px] max-w-[95vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
          <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <VenueLogo venueId={(opp.buy_venue_id ?? opp.buy_venue ?? "").toLowerCase()} name={opp.buy_venue} size={16} />
              <span className="text-sm font-bold font-sans text-slate-900 truncate">
                {opp.symbol} — buy {opp.buy_venue} / sell {opp.sell_venue}
              </span>
              <VenueLogo venueId={(opp.sell_venue_id ?? opp.sell_venue ?? "").toLowerCase()} name={opp.sell_venue} size={16} />
              <span
                className={`px-2 py-0.5 rounded-md text-xs font-sans font-semibold border ${
                  opp.is_executable
                    ? "bg-profit-dim text-profit border-profit-border"
                    : "bg-surface-raised text-slate-400 border-surface-border"
                }`}
              >
                {opp.is_executable ? "EXECUTABLE" : "SUB-HURDLE"}
              </span>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="p-1.5 rounded-lg text-slate-400 hover:text-slate-700 hover:bg-slate-100 transition cursor-pointer"
              title="Close details (ESC)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-5 space-y-5">
            <DetailSection title="Route">
              <DetailField
                label="Buy price"
                value={formatCurrencyPrice(opp.buy_price)}
                tip={`Best ask on ${opp.buy_venue} at scan time.`}
              />
              <DetailField
                label="Sell price"
                value={formatCurrencyPrice(opp.sell_price)}
                tip={`Best bid on ${opp.sell_venue} at scan time.`}
              />
              <DetailField
                label="Order qty"
                value={opp.order_qty.toString()}
                tip="Base-unit quantity the scan sized this route at."
              />
            </DetailSection>

            <DetailSection title="Economics">
              <DetailField
                label="Gross spread"
                value={`+${opp.gross_spread_bps.toFixed(1)} bps`}
                tip="Top-of-book cross-venue spread before any friction."
              />
              <DetailField
                label="Total fees"
                value={`-${opp.fee_bps.toFixed(1)} bps`}
                tip={`Round-trip commission: buy ${opp.buy_fee_bps.toFixed(1)} + sell ${opp.sell_fee_bps.toFixed(1)} bps.`}
              />
              <DetailField
                label="Net spread"
                value={`${opp.net_spread_bps >= 0 ? "+" : ""}${opp.net_spread_bps.toFixed(2)} bps`}
                tip="Gross minus ALL expected friction (fees + entry/exit slippage + closing spread)."
              />
              <DetailField
                label="Est. profit"
                value={formatCurrencyPnl(opp.estimated_profit_usd)}
                tip={`Net bps applied to the scan order qty of ${opp.order_qty}.`}
              />
              <DetailField
                label="Win %"
                value={
                  opp.win_probability != null
                    ? `${fmtPct(opp.win_probability)}${opp.win_probability_lower != null && opp.win_probability_upper != null ? ` (${(opp.win_probability_lower * 100).toFixed(0)}–${(opp.win_probability_upper * 100).toFixed(0)}%)` : ""}`
                    : "—"
                }
                tip="Empirical convergence probability from the pair's episode history, scored to the monitor's standard (min-episodes gate + OU fallback). Wilson 90% interval in parens. Informational only."
              />
              <DetailField
                label="Required gross"
                value={`${opp.required_gross_bps.toFixed(1)} bps`}
                tip="Gross spread needed for the trade to clear every friction component."
              />
            </DetailSection>

            <DetailSection title="Friction & Gates">
              <DetailField
                label="Entry slippage"
                value={`${opp.entry_slippage_bps.toFixed(1)} bps`}
                tip="VWAP-vs-top cost of entering both legs at the order qty."
              />
              <DetailField
                label="Exit slippage"
                value={`${opp.exit_slippage_bps.toFixed(1)} bps`}
                tip="Modelled VWAP cost of unwinding both legs."
              />
              <DetailField
                label="Closing spread"
                value={`${opp.closing_spread_bps.toFixed(1)} bps`}
                tip="Bid-ask crossing cost paid again on exit."
              />
              <DetailField
                label="Expected friction"
                value={`${opp.expected_friction_bps.toFixed(1)} bps`}
                tip="Total per-unit cost model: fees + entry + exit slippage + closing spread."
              />
              <DetailField
                label="Slippage gate"
                value={gate(opp.slippage_ok)}
                tip="Entry slippage within the configured max."
              />
              <DetailField
                label="Depth gate"
                value={gate(opp.depth_ok)}
                tip="Visible book depth covers the order qty at the required ratio."
              />
            </DetailSection>

            <DetailSection
              title="Capacity"
              tip="Profit vs position size walked through both leg orderbooks. Expected = win-probability-weighted; Net = raw capture. Cumulative = total at that size; Marginal = per-unit rate at that depth. X runs to the depth where marginal profit turns non-positive."
            >
              <DetailField
                label="Expected capacity"
                value={fmtUsd(opp.expected_capacity_usd)}
                tip="Probability-weighted fillable notional across book levels with positive marginal expected net (p·dev⁺ − friction). '—' when no win-rate curve exists."
              />
              <DetailField
                label="Optimal size"
                value={fmtUsd(opp.optimal_size_usd)}
                tip="Fillable size maximizing expected USD — the sweet spot of the capacity curve."
              />
              <DetailField
                label="Breakeven size"
                value={fmtUsd(opp.breakeven_size_usd)}
                tip="Largest fillable size whose marginal level still clears the win-probability floor with positive expected net."
              />
              <div className="col-span-full pt-1">
                {(opp.depth_curve?.length ?? 0) >= 2 ? (
                  <CapacityChart
                    points={opp.depth_curve ?? []}
                    optimalSizeUsd={opp.optimal_size_usd}
                    breakevenSizeUsd={opp.breakeven_size_usd}
                  />
                ) : (
                  <span className="text-slate-400 text-xs">no depth data</span>
                )}
              </div>
            </DetailSection>
          </div>

          <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
            <ExecuteButton
              executing={executing}
              disabled={!opp.is_executable || blocked}
              onClick={() => onExecute(opp)}
              title={blocked ? "symbol blocked: awaiting reconciliation" : "execute spatial route"}
              className="!text-xs !py-2.5 !px-2.5 cursor-pointer"
            />
          </div>
        </div>
      </div>
    );
  },
);

const LiveMatrixInner: React.FC<LiveMatrixProps> = ({
  symbol,
  orderbooks,
  opportunities,
  onRefresh,
  onExecuteTrade,
  loading,
  executionLogs = [],
  onOpenTickerConfig,
  activeVenueCodes,
  availableTickers,
  isTickerEnabled = false,
  onToggleTicker,
  isPaused = false,
  positionState = null,
  tickerExplorer,
  futuresParams = null,
  totalEquityUsd = null,
}) => {
  // Empty-string alias for internal computations; `symbol` stays nullable so
  // "no selection" is distinguishable from a real ticker.
  const sym = symbol ?? "";
  const rawTicker = useMemo(
    () =>
      (availableTickers ?? PERP_TICKERS).find((t) => t.symbol === symbol) || {
        symbol: sym,
        base: sym.split("-")[0],
        quote: sym.split("-")[1] || "USDC",
        name: sym,
        venues: ["HL", "LGT", "VNT"],
        basePrice: 100,
      },
    [availableTickers, symbol, sym],
  );

  const activeVenues = useMemo(() => {
    return activeVenueCodes ? getTickerActiveVenues(rawTicker, activeVenueCodes) : rawTicker.venues;
  }, [rawTicker, activeVenueCodes]);

  const activeTicker = useMemo(() => {
    return {
      ...rawTicker,
      venues: activeVenues,
    };
  }, [rawTicker, activeVenues]);

  // A ticker is only real when the user selected one AND at least 2 of its
  // venues are connected — otherwise the panel shows N/A instead of
  // fabricated defaults.
  const hasActiveTicker = symbol !== null && activeVenues.length >= 2;

  // State
  const [orderQty, setOrderQty] = useState(() => getDefaultOrderQty(activeTicker.basePrice));
  const [minBps, setMinBps] = useState(1.0);
  const [selectedPair, setSelectedPair] = useState<{ rowId: string; colId: string } | null>(null);
  const [detailOpp, setDetailOpp] = useState<SpatialOpportunityDto | null>(null);
  const [executing, setExecuting] = useState(false);
  const [internalConfigModalOpen, setInternalConfigModalOpen] = useState(false);
  const lastAutoExecTimeRef = React.useRef<number>(0);
  const handleInternalConfigClose = useCallback(() => setInternalConfigModalOpen(false), []);

  // Universal Simple Spread strategy params (shared across all tickers) +
  // per-ticker runtime grid state (accumulated units) keyed by symbol.
  const [loadedFuturesParams, setLoadedFuturesParams] = useState<StrategyParamsDto | null>(null);
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(() => new Map());
  const effectiveFuturesParams = futuresParams ?? loadedFuturesParams;

  const currentStrategyConfig: TickerStrategyConfigDto = useMemo(() => {
    const existing = strategyConfigs.get(sym);
    const sizing = deriveStepSizing(sym, activeTicker.venues, activeTicker.basePrice, totalEquityUsd, {
      stepSizeMult: effectiveFuturesParams?.stepSizeMult ?? 10,
      maxStepUsd: effectiveFuturesParams?.maxStepUsd ?? 2500,
      maxTickerEquityPct: effectiveFuturesParams?.maxTickerEquityPct ?? 20,
    });
    return {
      symbol: sym,
      targetProfitBps: effectiveFuturesParams?.targetProfitBps ?? minBps,
      windowPercentile: effectiveFuturesParams?.windowPercentile ?? 50,
      maxAccumulatedUnits: sizing.maxSteps,
      currentAccumulatedUnits: existing?.currentAccumulatedUnits ?? 0,
      executionMode: effectiveFuturesParams?.executionMode ?? "SINGLE_WAY",
      unitSize: sizing.stepBaseQty,
      userTargetLeverage: effectiveFuturesParams?.userTargetLeverage ?? 5,
      slidingWindow: effectiveFuturesParams?.slidingWindow ?? "24h",
    };
  }, [sym, strategyConfigs, effectiveFuturesParams, activeTicker.venues, activeTicker.basePrice, minBps, totalEquityUsd]);

  // Bot lifecycle for this ticker+strategy: Start / Pause / Reduce Only /
  // Stop (emergency flatten to delta-neutral via the signal ledger).
  const bot = useBotLifecycle({
    strategyId: "futures",
    symbol,
    externalOpenPositions: currentStrategyConfig.currentAccumulatedUnits > 0,
  });

  const handleBotStart = useCallback(() => {
    if (!isTickerEnabled && onToggleTicker) {
      onToggleTicker(sym, true);
    }
    bot.start();
  }, [isTickerEnabled, onToggleTicker, sym, bot.start]);

  const slidingWindowMode: "manual" | "auto" =
    effectiveFuturesParams?.slidingWindowMode === "auto" ? "auto" : "manual";
  const manualWindowSecs = useMemo(() => {
    const secs = effectiveFuturesParams?.slidingWindowSecs;
    if (typeof secs === "number" && Number.isFinite(secs) && secs >= 300 && secs <= 2_592_000) {
      return secs;
    }
    return parseDuration(effectiveFuturesParams?.slidingWindow ?? "") ?? 86_400;
  }, [effectiveFuturesParams]);

  const [baselineSnapshot, setBaselineSnapshot] = useState<MatrixBaselineSnapshotDto | null>(null);
  useEffect(() => {
    if (symbol === null) {
      setBaselineSnapshot(null);
      return;
    }
    let stopped = false;
    const load = async () => {
      try {
        const snap = await api.getMatrixBaselines(
          sym,
          slidingWindowMode,
          manualWindowSecs,
          currentStrategyConfig.windowPercentile
        );
        if (!stopped) setBaselineSnapshot(snap);
      } catch {
        if (!stopped) setBaselineSnapshot(null);
      }
    };
    load();
    const iv = setInterval(load, 15_000);
    return () => {
      stopped = true;
      clearInterval(iv);
    };
  }, [symbol, sym, slidingWindowMode, manualWindowSecs, currentStrategyConfig.windowPercentile]);

  const handleOpenConfig = useCallback(() => {
    if (onOpenTickerConfig) {
      onOpenTickerConfig(sym, "futures");
    } else {
      setInternalConfigModalOpen(true);
    }
  }, [onOpenTickerConfig, sym]);

  const handleUpdateTickerConfig = useCallback((cfg: TickerStrategyConfigDto) => {
    setStrategyConfigs((prev) => {
      const next = new Map(prev);
      next.set(cfg.symbol, cfg);
      return next;
    });
    api.saveTickerStrategyConfig(cfg).catch((err) => {
      console.warn("Failed to persist ticker grid state:", err);
    });
  }, []);

  const handleSaveStrategyParams = useCallback(
    (params: StrategyParamsDto) => {
      setLoadedFuturesParams(params);
      setMinBps(params.targetProfitBps);
    const sizing = deriveStepSizing(sym, activeTicker.venues, activeTicker.basePrice, totalEquityUsd, {
      stepSizeMult: params.stepSizeMult,
      maxStepUsd: params.maxStepUsd,
      maxTickerEquityPct: params.maxTickerEquityPct,
    });
    setOrderQty(sizing.stepBaseQty);
    api.saveStrategyParams(params).catch((err) => {
      console.warn("Failed to persist strategy params:", err);
    });
    },
    [sym, activeTicker.venues, activeTicker.basePrice, totalEquityUsd],
  );

  // Load universal futures params once
  useEffect(() => {
    let isMounted = true;
    api
      .getStrategyParams("futures")
      .then((p) => {
        if (!isMounted) return;
        setLoadedFuturesParams(p);
        setMinBps(p.targetProfitBps);
      })
      .catch((err) => {
        console.warn("Failed to fetch strategy params:", err);
      });
    return () => {
      isMounted = false;
    };
  }, []);

  // Keep orderQty in sync with the derived per-ticker step size
  useEffect(() => {
    const sizing = deriveStepSizing(sym, activeTicker.venues, activeTicker.basePrice, totalEquityUsd, {
      stepSizeMult: effectiveFuturesParams?.stepSizeMult ?? 10,
      maxStepUsd: effectiveFuturesParams?.maxStepUsd ?? 2500,
      maxTickerEquityPct: effectiveFuturesParams?.maxTickerEquityPct ?? 20,
    });
    setOrderQty(sizing.stepBaseQty);
  }, [sym, activeTicker.venues, activeTicker.basePrice, effectiveFuturesParams, totalEquityUsd]);

  // Load per-ticker grid state (accumulated units) for the active symbol
  useEffect(() => {
    if (symbol === null) return;
    let isMounted = true;
    api
      .getTickerStrategyConfig(sym)
      .then((cfg) => {
        if (isMounted && cfg) {
          setStrategyConfigs((prev) => {
            const next = new Map(prev);
            next.set(sym, cfg);
            return next;
          });
        }
      })
      .catch((err) => {
        console.warn("Failed to fetch ticker strategy config:", err);
      });
    return () => {
      isMounted = false;
    };
  }, [symbol]);

  const windowSeconds = baselineSnapshot?.effective_window_secs ?? manualWindowSecs;
  const windowLabel = formatDuration(windowSeconds);
  const windowTitle =
    slidingWindowMode === "auto"
      ? baselineSnapshot?.auto_window_secs != null
        ? `Auto window · ${windowLabel} from ${baselineSnapshot.qualifying_pair_count} qualified venue pairs`
        : `Auto collecting history · manual fallback ${formatDuration(manualWindowSecs)}`
      : `Manual rolling ${windowLabel} baseline`;

  // Build Cross-Venue Matrix
  const venueMatrix = useMemo(() => {
    if (orderbooks.length === 0) {
      return { venues: [], venueLabels: [], grid: [] };
    }
    return buildCrossVenueMatrix(
      sym,
      orderbooks,
      windowSeconds,
      orderQty,
      minBps,
      currentStrategyConfig,
      globalSpreadTracker,
      baselineSnapshot?.pairs ?? [],
      opportunities
    );
  }, [symbol, orderbooks, windowSeconds, orderQty, minBps, currentStrategyConfig, baselineSnapshot, opportunities]);

  // Derive selected cell cleanly from selectedPair coordinates and current active matrix grid
  const selectedCell = useMemo<MatrixCellDto | null>(() => {
    if (!selectedPair) return null;
    for (const row of venueMatrix.grid) {
      for (const cell of row) {
        if (cell.rowId === selectedPair.rowId && cell.colId === selectedPair.colId) {
          return cell;
        }
      }
    }
    return null;
  }, [selectedPair, venueMatrix.grid]);

  // Execution handler
  const handleExecuteCellTrade = useCallback(
    async (cell: MatrixCellDto) => {
    if (cell.isDiagonal || executing) return;
    // Entries blocked unless idle/running; unwinds allowed except mid-stop.
    if (cell.isConvergenceUnwind) {
      if (bot.phase === "stopping") return;
    } else if (bot.phase !== "idle" && bot.phase !== "running") {
      return;
    }
    setExecuting(true);

    try {
      const matchedRoute = opportunities.find(
        (r) =>
          r.buy_venue_id.toLowerCase() === cell.rowId.toLowerCase() &&
          r.sell_venue_id.toLowerCase() === cell.colId.toLowerCase()
      );
      const opp: SpatialOpportunityDto = {
        id: `${cell.longSymbol}-${cell.rowId}-${cell.colId}`,
        symbol: cell.longSymbol,
        buy_venue: cell.longVenue,
        sell_venue: cell.shortVenue,
        buy_price: cell.longPrice,
        sell_price: cell.shortPrice,
        gross_spread: cell.shortPrice - cell.longPrice,
        gross_spread_bps: cell.grossSpreadBps,
        fee_bps: cell.feeBps,
        net_spread_bps: cell.netSpreadBps,
        order_qty: cell.orderQty,
        estimated_profit_usd: cell.estProfitUsd,
        is_executable: cell.isExecutable,
        buy_venue_id: cell.rowId,
        sell_venue_id: cell.colId,
        buy_fee_bps: matchedRoute?.buy_fee_bps ?? 0,
        sell_fee_bps: matchedRoute?.sell_fee_bps ?? 0,
        round_trip_commission_bps: cell.roundTripCommissionBps ?? 0,
        entry_slippage_bps: cell.entrySlippageBps ?? 0,
        exit_slippage_bps: cell.exitSlippageBps ?? 0,
        closing_spread_bps: cell.closingFrictionBps,
        expected_friction_bps: cell.expectedFrictionBps ?? 0,
        required_gross_bps: cell.requiredGrossBps ?? 0,
        slippage_ok: cell.slippageOk ?? false,
        depth_ok: cell.depthOk ?? false,
        direction: cell.isConvergenceUnwind ? "Convergence Unwind" : "Divergence Entry",
        trigger_spread_bps: cell.netSpreadBps,
        baseline_mean_spread_bps: cell.baselineReady === false ? undefined : cell.baselineMeanBps,
        position_step: cell.isConvergenceUnwind
          ? currentStrategyConfig.currentAccumulatedUnits
          : currentStrategyConfig.currentAccumulatedUnits + 1,
        max_units: currentStrategyConfig.maxAccumulatedUnits,
        nominal_hurdle_bps: cell.costReady ? cell.requiredGrossBps : undefined,
      };

      await onExecuteTrade(opp);

      // Track accumulated grid steps — per-ticker runtime state
      if (cell.isConvergenceUnwind) {
        handleUpdateTickerConfig({
          ...currentStrategyConfig,
          currentAccumulatedUnits: Math.max(0, currentStrategyConfig.currentAccumulatedUnits - 1),
        });
      } else {
        handleUpdateTickerConfig({
          ...currentStrategyConfig,
          currentAccumulatedUnits: Math.min(
            currentStrategyConfig.maxAccumulatedUnits,
            currentStrategyConfig.currentAccumulatedUnits + 1
          ),
        });
      }
    } catch (err: unknown) {
      console.error("Auto-execution trade dispatch error:", err);
    } finally {
      setExecuting(false);
    }
    },
    [
      executing,
      bot.phase,
      opportunities,
      currentStrategyConfig,
      onExecuteTrade,
      handleUpdateTickerConfig,
    ],
  );

  const handleSelectCell = useCallback((rowId: string, colId: string) => {
    setSelectedPair({ rowId, colId });
  }, []);

  const handleExecuteOpp = useCallback(
    (opp: SpatialOpportunityDto) => {
      const enrichedOpp: SpatialOpportunityDto = {
        ...opp,
        direction: "Divergence Entry",
        trigger_spread_bps: opp.net_spread_bps,
        baseline_mean_spread_bps: currentStrategyConfig.targetProfitBps,
        position_step: currentStrategyConfig.currentAccumulatedUnits + 1,
        max_units: currentStrategyConfig.maxAccumulatedUnits,
        nominal_hurdle_bps: opp.required_gross_bps,
      };
      onExecuteTrade(enrichedOpp);
    },
    [currentStrategyConfig, onExecuteTrade],
  );

  const handleHistoryRefresh = useCallback(() => {
    onRefresh(orderQty, minBps);
  }, [onRefresh, orderQty, minBps]);

  const blockingIds = useMemo(
    () => positionState?.blockingSignalIds.join(", ") ?? "",
    [positionState],
  );

  // Automated Execution Engine Loop for Futures Spread Convergence.
  // Entries require phase=running; convergence unwinds also run in
  // reduce-only (they are the exits that drain the grid).
  useEffect(() => {
    if (!bot.exitsAllowed || executing || !isTickerEnabled || isPaused) return;
    if (positionState?.blocked) return; // signal still reconciling — no new orders

    const now = Date.now();
    if (now - lastAutoExecTimeRef.current < 4000) return; // 4s cooldown between auto-trades

    let bestCell: MatrixCellDto | null = null;
    let highestNet = -Infinity;

    for (const row of venueMatrix.grid) {
      for (const cell of row) {
        if (cell.isDiagonal) continue;
        if (bot.entriesAllowed && !isPositionReduceOnly(sym) && cell.isExecutable && cell.netSpreadBps > highestNet) {
          if (currentStrategyConfig.currentAccumulatedUnits < currentStrategyConfig.maxAccumulatedUnits) {
            highestNet = cell.netSpreadBps;
            bestCell = cell;
          }
        } else if (cell.isConvergenceUnwind && currentStrategyConfig.currentAccumulatedUnits > 0) {
          bestCell = cell;
          break;
        }
      }
      if (bestCell?.isConvergenceUnwind) break;
    }

    if (bestCell) {
      lastAutoExecTimeRef.current = now;
      handleExecuteCellTrade(bestCell);
    }
  }, [bot.entriesAllowed, bot.exitsAllowed, executing, venueMatrix.grid, currentStrategyConfig, isTickerEnabled, isPaused, positionState?.blocked, handleExecuteCellTrade]);

  // Best Bid & Ask across all venues for Consolidated Level 2 Quote Board
  const overallBestBid = useMemo(() => {
    if (!orderbooks || orderbooks.length === 0) return 0;
    return Math.max(...orderbooks.map((b) => b.best_bid || 0));
  }, [orderbooks]);

  const overallBestAsk = useMemo(() => {
    if (!orderbooks || orderbooks.length === 0) return 0;
    const asks = orderbooks.map((b) => b.best_ask).filter((a) => a > 0);
    return asks.length > 0 ? Math.min(...asks) : 0;
  }, [orderbooks]);


  const [bottomTab, setBottomTab] = useState<"history" | "opps" | "logs">("history");
  const [stageTab, setStageTab] = useState<"matrix" | "quotes" | "opportunities">("matrix");

  const tickerLogsCount = useMemo(() => {
    const symKey = sym.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];
    return (executionLogs || []).filter((l) => {
      if (!l.symbol) return false;
      const logKey = l.symbol.toUpperCase();
      return logKey === symKey || logKey.split(/[-_/]/)[0] === baseKey;
    }).length;
  }, [executionLogs, symbol]);

  // Ranked Opportunities shows profitable routes only (net spread > 0),
  // mirroring the Spread Radar positive-only convention. This single memo
  // feeds both the tab count badge and the table.
  const profitableOpportunities = useMemo(
    () => opportunities.filter((o) => Number.isFinite(o.net_spread_bps) && o.net_spread_bps > 0),
    [opportunities],
  );

  return (
    <div className="p-4 h-full flex flex-col space-y-4 overflow-hidden text-slate-900 font-sans">
      {/* 1. Strategy Ribbon — parameters left, configurator + lifecycle right
             (same layout as the other strategy pages) */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>

          {/* Delta-neutral gate: ticker refuses new orders until the open
              signal's own legs reconcile. */}
          {hasActiveTicker && positionState?.blocked && (
            <div
              className="flex items-center space-x-1.5 px-2.5 py-1.5 rounded-lg border bg-warn-dim text-warn border-warn-border"
              title={`Signal ${positionState.blockingSignalIds.join(", ")} is in-flight or imbalanced — new orders on ${symbol} are refused until its legs confirm delta-neutral. Net filled position: ${positionState.netPositionQty}`}
            >
              <span className="w-1.5 h-1.5 rounded-full bg-warn animate-pulse" />
              <span className="font-bold text-xs tracking-wide">
                Reconciling{positionState.netPositionQty !== 0 ? ` (${positionState.netPositionQty > 0 ? "+" : ""}${positionState.netPositionQty.toFixed(4)} net)` : ""}
              </span>
            </div>
          )}

          {/* Strategy parameters — hidden when no ticker is connected;
              the defaults would be fabricated. */}
          {hasActiveTicker && (
            <>
              <div
                className="ribbon-pill"
                title="Target profit locked in per round-trip after fees and friction — edit via Configurator"
              >
                <span className="text-slate-400">Target</span>
                <span className="font-bold text-slate-900">{currentStrategyConfig.targetProfitBps.toFixed(1)}</span>
                <span className="text-slate-400">BPS</span>
              </div>

              <div
                className="ribbon-pill"
                title={`${windowTitle} · percentile trigger P${currentStrategyConfig.windowPercentile}%`}
              >
                <span className="text-slate-400">Window</span>
                <span className="font-bold text-slate-900">{windowLabel}</span>
                <span className="text-slate-400">P{currentStrategyConfig.windowPercentile}%</span>
              </div>

            </>
          )}
        </div>

        {/* Right: Configure Strategy + Bot Lifecycle */}
        <div className="flex items-center space-x-3 shrink-0">
          {/* Configure Strategy Button — universal params, no ticker required */}
          <button
            type="button"
            onClick={handleOpenConfig}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Simple Spread strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configurator</span>
          </button>

          <BotLifecycleControls
            phase={bot.phase}
            hasOpenPositions={bot.hasOpenPositions}
            disabled={!hasActiveTicker}
            onStart={handleBotStart}
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

      {/* 2. Page Body — ticker selector column on the left, strategy content
             scrolls in the right column. */}
      <div className="flex-1 min-h-0 flex gap-4">
        {tickerExplorer && (
              <div className="w-64 shrink-0 min-h-0">{tickerExplorer}</div>
        )}
        <div className="flex-1 min-w-0 overflow-y-auto space-y-4 pr-4">
          {/* Execution Telemetry — post-trade diagnosis for the selected
              ticker, directly beneath the Strategy Parameters ribbon. */}
          <TickerTelemetryGadget
            symbol={sym}
            logs={executionLogs || []}
            gridCurrentUnits={currentStrategyConfig.currentAccumulatedUnits}
            gridMaxUnits={currentStrategyConfig.maxAccumulatedUnits}
          />

          {/* Signal Trend — standalone gadget above the matrix stage. */}
          <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs">
            <SignalTrendGadget
              symbol={sym}
              botWindowSec={windowSeconds}
              hurdleBps={minBps}
            />
          </div>

          {/* 3. Main Stage: Dislocation Matrix + Venue Quotes + Ranked Opportunities — one tabbed gadget */}
          <div className="bg-surface rounded-lg border border-surface-border shadow-2xs overflow-hidden">
            {/* Tab switcher — same chrome as the bottom dock */}
            <div className="flex items-stretch border-b border-surface-border bg-slate-50/80 select-none">
              <div className="grid grid-cols-3 flex-1 divide-x divide-surface-border">
                <button
                  type="button"
                  onClick={() => setStageTab("matrix")}
                  className={`relative h-9 flex items-center justify-center space-x-2 px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
                    stageTab === "matrix"
                      ? "text-accent font-bold bg-surface"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
                  }`}
                >
                  <LayoutGrid
                    className={`w-3 h-3 transition shrink-0 ${
                      stageTab === "matrix" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                    }`}
                  />
                  <span>Dislocation Matrix</span>
                  {stageTab === "matrix" && (
                    <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
                  )}
                </button>

                <button
                  type="button"
                  onClick={() => setStageTab("quotes")}
                  className={`relative h-9 flex items-center justify-center space-x-2 px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
                    stageTab === "quotes"
                      ? "text-accent font-bold bg-surface"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
                  }`}
                >
                  <Table
                    className={`w-3 h-3 transition shrink-0 ${
                      stageTab === "quotes" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                    }`}
                  />
                  <span>Venue Quotes</span>
                  {orderbooks.length > 0 && (
                  <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                    {orderbooks.length}
                  </span>
                  )}
                  {stageTab === "quotes" && (
                    <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
                  )}
                </button>

                <button
                  type="button"
                  onClick={() => setStageTab("opportunities")}
                  className={`relative h-9 flex items-center justify-center gap-2 px-2.5 text-[11px] font-sans transition select-none group cursor-pointer ${
                    stageTab === "opportunities"
                      ? "text-accent font-bold bg-surface"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
                  }`}
                >
                  <Zap
                    className={`w-3 h-3 transition shrink-0 ${
                      stageTab === "opportunities" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                    }`}
                  />
                  <span>Ranked Opportunities</span>
                  {profitableOpportunities.length > 0 && (
                  <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                    {profitableOpportunities.length}
                  </span>
                  )}
                  {stageTab === "opportunities" && (
                    <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
                  )}
                </button>
              </div>
            </div>

            <div className="p-3.5 space-y-3">
              {stageTab === "matrix" ? (
                <>
          {/* Matrix Table */}
          <div className="overflow-auto max-h-[320px] rounded-lg border border-surface-border">
            {venueMatrix.grid.length < 2 ? (
              symbol === null ? (
                <EmptyState
                  title="No Ticker Selected"
                  hint="Select a market in the explorer to stream live venue quotes."
                />
              ) : (
                <EmptyState
                  title="Cross-Venue Arbitrage Unavailable"
                  hint="Requires at least 2 enabled & connected exchanges for this ticker — configure venues in System Settings."
                />
              )
            ) : (
                <table className="w-max border-collapse font-mono text-[10px] table-fixed">
                  <colgroup>
                    <col className="w-[122px]" />
                    {venueMatrix.venueLabels.map((colVenue) => (
                      <col key={colVenue} className="w-[130px]" />
                    ))}
                  </colgroup>
                  <thead className="sticky top-0 z-20 bg-surface-raised text-slate-500 text-[10px] font-semibold tracking-wider">
                    <tr className="h-11 bg-surface-raised border-b border-surface-border">
                      <th className="w-[122px] p-1.5 text-left font-semibold text-slate-500 text-[10px] tracking-wider border-r border-b border-surface-border whitespace-nowrap overflow-hidden sticky top-0 left-0 bg-surface-raised z-30">
                        <div className="flex items-center justify-between whitespace-nowrap overflow-hidden">
                          <span className="truncate">LONG ↓</span>
                          <span className="truncate">SHORT →</span>
                        </div>
                      </th>
                      {venueMatrix.venueLabels.map((colVenue, j) => {
                        const sellPrice = orderbooks[j]?.best_bid ?? 0;
                        return (
                          <th
                            key={colVenue}
                            className="w-[130px] p-1.5 text-center font-bold text-slate-800 border-r border-b border-surface-border last:border-r-0 whitespace-nowrap overflow-hidden sticky top-0 bg-surface-raised z-20"
                          >
                            <div className="text-[10px] text-slate-900 truncate flex items-center justify-center gap-1">
                              <VenueLogo venueId={(venueMatrix.venues[j] ?? colVenue).toLowerCase()} name={colVenue} size={14} />
                              <span className="truncate">{colVenue}</span>
                            </div>
                            <div className="text-[10px] font-normal text-profit mt-0.5 truncate">
                              Sell @ {formatCurrencyPrice(sellPrice)}
                            </div>
                          </th>
                        );
                      })}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-surface-border">
                    {venueMatrix.grid.map((row, i) => {
                      const rowLabel = venueMatrix.venueLabels[i];
                      const buyPrice = orderbooks[i]?.best_ask ?? 0;

                      return (
                        <tr key={venueMatrix.venues[i]} className="h-10 hover:bg-surface-raised/40 transition">
                          {/* Row Header (Long Leg) */}
                          <td className="h-10 p-1.5 font-semibold text-slate-900 border-r border-surface-border bg-surface-raised/95 sticky left-0 z-10 whitespace-nowrap overflow-hidden">
                            <div className="text-[10px] font-bold text-slate-900 truncate flex items-center gap-1">
                              <VenueLogo venueId={(venueMatrix.venues[i] ?? rowLabel).toLowerCase()} name={rowLabel} size={14} />
                              <span className="truncate">{rowLabel}</span>
                            </div>
                            <div className="text-[10px] font-normal text-loss mt-0.5 truncate">
                              Buy @ {formatCurrencyPrice(buyPrice)}
                            </div>
                          </td>

                          {/* Cells */}
                          {row.map((cell) => (
                            <MatrixCellView
                              key={`${cell.rowId}-${cell.colId}`}
                              cell={cell}
                              isSelected={
                                selectedCell?.rowId === cell.rowId &&
                                selectedCell?.colId === cell.colId
                              }
                              minBps={minBps}
                              onSelect={handleSelectCell}
                            />
                          ))}
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
          </div>
                </>
              ) : stageTab === "quotes" ? (
                <>

            {/* Consolidated Level 2 Table */}
            <div className="overflow-x-auto rounded-lg border border-surface-border">
              <table className="w-full text-left font-mono text-xs">
                <thead className="bg-surface-raised text-xs text-slate-500 tracking-wider border-b border-surface-border">
                  <tr>
                    <th className="py-2 px-2.5">Venue</th>
                    <th className="py-2 px-2.5">Bid (Sell)</th>
                    <th className="py-2 px-2.5">Ask (Buy)</th>
                    <th className="py-2 px-2 text-right">Spread</th>
                    <th className="py-2 px-2.5 text-right">8h Fund</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border text-xs">
                  {orderbooks.length === 0 && (
                    <tr>
                      <td colSpan={5}>
                        {symbol === null ? (
                          <EmptyState
                            title="No Ticker Selected"
                            hint="Select a market in the explorer to stream live venue quotes."
                          />
                        ) : (
                          <EmptyState
                            title="No Venue Quotes"
                            hint="Enable and connect at least one exchange in System Settings."
                          />
                        )}
                      </td>
                    </tr>
                  )}
                  {orderbooks.map((book) => {
                    const isBestBid = book.best_bid === overallBestBid && overallBestBid > 0;
                    const isBestAsk = book.best_ask === overallBestAsk && overallBestAsk > 0;
                    const spreadColor = book.spread_bps < 10 ? "text-profit" : "text-warn";

                    return (
                      <tr key={book.venue_id} className="hover:bg-surface-raised/50 transition">
                        <td className="py-2 px-2.5 font-bold text-slate-900">
                          <span className="flex items-center gap-1.5 min-w-0">
                            <VenueLogo venueId={(book.venue_id ?? book.venue_name ?? "").toLowerCase()} name={book.venue_name} size={16} />
                            <span className="truncate">{book.venue_name}</span>
                          </span>
                        </td>
                        <td className="py-2 px-2.5 text-profit font-semibold">
                          <span className="flex items-center space-x-1">
                            <span>{formatCurrencyPrice(book.best_bid)}</span>
                            {isBestBid && (
                              <span className="px-1 py-0.2 rounded-lg text-xs font-bold bg-profit-dim text-profit border border-profit-border">
                                Best
                              </span>
                            )}
                          </span>
                        </td>
                        <td className="py-2 px-2.5 text-loss font-semibold">
                          <span className="flex items-center space-x-1">
                            <span>{formatCurrencyPrice(book.best_ask)}</span>
                            {isBestAsk && (
                              <span className="px-1 py-0.2 rounded-lg text-xs font-bold bg-loss-dim text-loss border border-loss-border">
                                Best
                              </span>
                            )}
                          </span>
                        </td>
                        <td className={`py-2 px-2 text-right font-medium ${spreadColor}`}>
                          {book.spread_bps.toFixed(1)} bps
                        </td>
                        <td className="py-2 px-2.5 text-right text-slate-600">
                          {book.funding_rate_8h_pct >= 0 ? "+" : ""}
                          {book.funding_rate_8h_pct.toFixed(4)}%
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
                </>
              ) : (
                <>
                  {profitableOpportunities.length === 0 ? (
                    <div className="border border-dashed border-surface-border rounded-lg">
                      <EmptyState
                        plain
                        title="No Profitable Opportunity"
                        hint="No routes with positive net spread — only profitable opportunities are listed."
                      />
                    </div>
                  ) : (
                    <div className="overflow-x-auto max-h-64 overflow-y-auto rounded-lg border border-surface-border">
                      <table className="w-full text-left text-xs font-sans">
                        <thead className="sticky top-0 bg-surface-raised border-b border-surface-border text-slate-500 text-xs tracking-wider">
                          <tr>
                            <th className="py-2 px-3">Route (Buy / Sell)</th>
                            <th className="py-2 px-3">Buy Price</th>
                            <th className="py-2 px-3">Sell Price</th>
                            <th className="py-2 px-3">Gross Spread</th>
                            <th className="py-2 px-3">Fees</th>
                            <th className="py-2 px-3">Net Spread</th>
                            <th className="py-2 px-3">Win %</th>
                            <th className="py-2 px-3">Est. Profit</th>
                            <th className="py-2 px-3 text-right">Action</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-surface-border">
                          {profitableOpportunities.map((opp) => (
                            <OpportunityRowView
                              key={opp.id}
                              opp={opp}
                              minBps={minBps}
                              blocked={positionState?.blocked ?? false}
                              blockingIds={blockingIds}
                              botPhase={bot.phase}
                              loading={loading}
                              symbolLabel={symbol}
                              onExecute={handleExecuteOpp}
                              onSelect={setDetailOpp}
                            />
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                </>
              )}
            </div>
          </div>

      {/* 4. Bottom Docked Tabbed Panel: Execution History vs Log Monitor */}
      <div className="bg-surface rounded-lg border border-surface-border shadow-2xs overflow-hidden">
        {/* Tab Switcher with dividers — three equal tabs */}
        <div className="flex items-stretch border-b border-surface-border bg-slate-50/80 select-none">
          <div className="grid grid-cols-3 flex-1 divide-x divide-surface-border">
            <button
              type="button"
              onClick={() => setBottomTab("history")}
              className={`relative h-9 flex items-center justify-center space-x-2 px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
                bottomTab === "history"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <History
                className={`w-3 h-3 transition shrink-0 ${
                  bottomTab === "history" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Execution History</span>
              {tickerLogsCount > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                {tickerLogsCount}
              </span>
              )}
              {bottomTab === "history" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>

            <button
              type="button"
              onClick={() => setBottomTab("opps")}
              className={`relative h-9 flex items-center justify-center space-x-2 px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
                bottomTab === "opps"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <Radar
                className={`w-3 h-3 transition shrink-0 ${
                  bottomTab === "opps" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Opportunity History</span>
              {bottomTab === "opps" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>

            <button
              type="button"
              onClick={() => setBottomTab("logs")}
              className={`relative h-9 flex items-center justify-center space-x-2 px-2.5 text-[11px] font-mono transition select-none group cursor-pointer ${
                bottomTab === "logs"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <Terminal
                className={`w-3 h-3 transition shrink-0 ${
                  bottomTab === "logs" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Log Monitor</span>
              {tickerLogsCount > 0 && (
              <span className="px-1.5 py-0.5 rounded-md text-[11px] font-sans font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
                {tickerLogsCount}
              </span>
              )}
              {bottomTab === "logs" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
          </div>
        </div>

        {/* Tab Content */}
        <div className="p-3.5">
          {bottomTab === "history" ? (
            <div className="max-h-96 overflow-y-auto">
              <TickerTransactionHistory
                symbol={sym}
                logs={executionLogs || []}
                config={currentStrategyConfig}
                onRefresh={handleHistoryRefresh}
              />
            </div>
          ) : bottomTab === "opps" ? (
            <div className="max-h-96 overflow-y-auto">
              <TickerOpportunityHistory symbol={sym} />
            </div>
          ) : (
            <div className="overflow-y-auto">
              <TickerLogMonitor
                symbol={sym}
                strategyName="Simple Spread"
                config={currentStrategyConfig}
                executionLogs={executionLogs || []}
                orderbooks={orderbooks}
                autoBotEnabled={bot.active}
                isPaused={isPaused}
              />
            </div>
          )}
        </div>
          </div>
        </div>
      </div>

      {/* Fallback Internal Config Modal */}
      {!onOpenTickerConfig && (
        <TickerConfigModal
          isOpen={internalConfigModalOpen}
          onClose={handleInternalConfigClose}
          tickerSymbol={sym}
          onSaveConfig={handleSaveStrategyParams}
          initialStrategy="futures"
        />
      )}

      {/* Opportunity detail modal — click any ranked row */}
      {detailOpp && (
        <SpatialDetailModal
          opp={detailOpp}
          executing={executing}
          blocked={positionState?.blocked ?? false}
          onExecute={(o) => {
            setDetailOpp(null);
            handleExecuteOpp(o);
          }}
          onClose={() => setDetailOpp(null)}
        />
      )}
    </div>
  );
};

export const LiveMatrix: React.FC<LiveMatrixProps> = React.memo(LiveMatrixInner);
