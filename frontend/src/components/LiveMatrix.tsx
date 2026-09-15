import React, { useState, useEffect, useMemo } from "react";
import {
  TrendingUp,
  TrendingDown,
  Zap,
  History,
  Terminal,
  SlidersHorizontal,
  ShieldCheck,
  LayoutGrid,
  Table,
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
import { computeSafeLeverage, deriveUnitSize } from "../services/contractSpecs";
import { InfoTooltip } from "./InfoTooltip";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { useBotLifecycle } from "../hooks/useBotLifecycle";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerTelemetryGadget } from "./TickerTelemetryGadget";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { SignalTrendGadget } from "./SignalTrendGadget";
import { TickerConfigModal } from "./TickerConfigModal";
import { EmptyState } from "./EmptyState";
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
  /** Left-column ticker selector gadget, composed by the parent — symbol
   *  selection is strategy-specific so it lives on this page, not the
   *  universal sidebar. */
  tickerExplorer?: React.ReactNode;
}

export const LiveMatrix: React.FC<LiveMatrixProps> = ({
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
}) => {
  // Empty-string alias for internal computations; `symbol` stays nullable so
  // "no selection" is distinguishable from a real ticker.
  const sym = symbol ?? "";
  const rawTicker = (availableTickers ?? PERP_TICKERS).find((t) => t.symbol === symbol) || {
    symbol: sym,
    base: sym.split("-")[0],
    quote: sym.split("-")[1] || "USDC",
    name: sym,
    venues: ["HL", "LGT", "VNT"],
    basePrice: 100,
  };

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
  const [executing, setExecuting] = useState(false);
  const [internalConfigModalOpen, setInternalConfigModalOpen] = useState(false);
  const lastAutoExecTimeRef = React.useRef<number>(0);

  // Universal Single Spread strategy params (shared across all tickers) +
  // per-ticker runtime grid state (accumulated units) keyed by symbol.
  const [loadedFuturesParams, setLoadedFuturesParams] = useState<StrategyParamsDto | null>(null);
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(() => new Map());
  const effectiveFuturesParams = futuresParams ?? loadedFuturesParams;

  const currentStrategyConfig: TickerStrategyConfigDto = useMemo(() => {
    const existing = strategyConfigs.get(sym);
    const derivedUnit = deriveUnitSize(sym, activeTicker.venues, activeTicker.basePrice, effectiveFuturesParams?.unitSizeUsd ?? 2500);
    return {
      symbol: sym,
      targetProfitBps: effectiveFuturesParams?.targetProfitBps ?? minBps,
      windowPercentile: effectiveFuturesParams?.windowPercentile ?? 50,
      maxAccumulatedUnits: effectiveFuturesParams?.maxAccumulatedUnits ?? 3,
      currentAccumulatedUnits: existing?.currentAccumulatedUnits ?? 0,
      executionMode: effectiveFuturesParams?.executionMode ?? "SINGLE_WAY",
      unitSize: derivedUnit,
      userTargetLeverage: effectiveFuturesParams?.userTargetLeverage ?? 5,
      slidingWindow: effectiveFuturesParams?.slidingWindow ?? "24h",
    };
  }, [sym, strategyConfigs, effectiveFuturesParams, activeTicker.venues, activeTicker.basePrice, minBps]);

  // Bot lifecycle for this ticker+strategy: Start / Pause / Reduce Only /
  // Stop (emergency flatten to delta-neutral via the signal ledger).
  const bot = useBotLifecycle({
    strategyId: "futures",
    symbol,
    externalOpenPositions: currentStrategyConfig.currentAccumulatedUnits > 0,
  });

  const handleBotStart = () => {
    if (!isTickerEnabled && onToggleTicker) {
      onToggleTicker(sym, true);
    }
    bot.start();
  };

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

  // Safe leverage calculation across venues
  const safeLeverage = useMemo(() => {
    return computeSafeLeverage(
      activeTicker.symbol,
      activeTicker.venues,
      activeTicker.basePrice,
      currentStrategyConfig.unitSize,
      currentStrategyConfig.userTargetLeverage
    );
  }, [activeTicker, currentStrategyConfig.unitSize, currentStrategyConfig.userTargetLeverage]);

  const handleOpenConfig = () => {
    if (onOpenTickerConfig) {
      onOpenTickerConfig(sym, "futures");
    } else {
      setInternalConfigModalOpen(true);
    }
  };

  const handleUpdateTickerConfig = (cfg: TickerStrategyConfigDto) => {
    setStrategyConfigs((prev) => {
      const next = new Map(prev);
      next.set(cfg.symbol, cfg);
      return next;
    });
    api.saveTickerStrategyConfig(cfg).catch((err) => {
      console.warn("Failed to persist ticker grid state:", err);
    });
  };

  const handleSaveStrategyParams = (params: StrategyParamsDto) => {
    setLoadedFuturesParams(params);
    setMinBps(params.targetProfitBps);
    setOrderQty(deriveUnitSize(sym, activeTicker.venues, activeTicker.basePrice, params.unitSizeUsd));
    api.saveStrategyParams(params).catch((err) => {
      console.warn("Failed to persist strategy params:", err);
    });
  };

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

  // Keep orderQty in sync with the derived per-ticker unit size
  useEffect(() => {
    setOrderQty(deriveUnitSize(sym, activeTicker.venues, activeTicker.basePrice, effectiveFuturesParams?.unitSizeUsd ?? 2500));
  }, [sym, activeTicker.venues, activeTicker.basePrice, effectiveFuturesParams]);

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

  // Derive execution stats for the currently selected ticker
  const tickerStats = useMemo(() => {
    let count = 0;
    let pnl = 0;
    const symKey = sym.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];

    for (const log of executionLogs) {
      if (!log.symbol) continue;
      const logKey = log.symbol.toUpperCase();
      const logBase = logKey.split(/[-_/]/)[0];
      if (logKey === symKey || logBase === baseKey) {
        count += 1;
        pnl += log.net_profit || 0;
      }
    }
    return { count, pnl: Math.round(pnl * 100) / 100 };
  }, [executionLogs, symbol]);

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
  const handleExecuteCellTrade = async (cell: MatrixCellDto) => {
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
  };

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
        if (bot.entriesAllowed && cell.isExecutable && cell.netSpreadBps > highestNet) {
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
  }, [bot.entriesAllowed, bot.exitsAllowed, executing, venueMatrix.grid, currentStrategyConfig, isTickerEnabled, isPaused, positionState?.blocked]);

  // Derive best off-diagonal opportunity from active matrix grid
  const bestMatrixCell = useMemo<MatrixCellDto | null>(() => {
    let best: MatrixCellDto | null = null;
    for (const row of venueMatrix.grid) {
      for (const cell of row) {
        if (cell.isDiagonal) continue;
        if (!best || cell.netSpreadBps > best.netSpreadBps) {
          best = cell;
        }
      }
    }
    return best;
  }, [venueMatrix.grid]);

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

  const overallBestSpreadBps = useMemo(() => {
    if (overallBestBid <= 0 || overallBestAsk <= 0) return 0;
    return ((overallBestBid - overallBestAsk) / overallBestAsk) * 10000;
  }, [overallBestBid, overallBestAsk]);

  const routeHurdleBps = useMemo<number | null>(() => {
    if (selectedCell?.costReady && selectedCell.requiredGrossBps != null) {
      return selectedCell.requiredGrossBps;
    }
    let min: number | null = null;
    for (const row of venueMatrix.grid) {
      for (const cell of row) {
        if (cell.isDiagonal || !cell.costReady || cell.requiredGrossBps == null) continue;
        if (min === null || cell.requiredGrossBps < min) min = cell.requiredGrossBps;
      }
    }
    return min;
  }, [selectedCell, venueMatrix.grid]);

  const routeHurdleTitle = useMemo(() => {
    if (selectedCell?.costReady && selectedCell.requiredGrossBps != null) {
      return `Backend route cost for ${selectedCell.rowLabel} → ${selectedCell.colLabel}: commission ${selectedCell.roundTripCommissionBps?.toFixed(1)} BPS + entry slippage ${selectedCell.entrySlippageBps?.toFixed(1)} BPS + exit slippage ${selectedCell.exitSlippageBps?.toFixed(1)} BPS + exit crossing ${selectedCell.closingFrictionBps.toFixed(1)} BPS + target ${currentStrategyConfig.targetProfitBps.toFixed(1)} BPS`;
    }
    if (routeHurdleBps != null) {
      return "Lowest required gross across costed routes — backend, current books";
    }
    return "No route selected — route economics unavailable";
  }, [selectedCell, routeHurdleBps, currentStrategyConfig.targetProfitBps]);

  const [bottomTab, setBottomTab] = useState<"opportunities" | "history" | "logs">("opportunities");
  const [stageTab, setStageTab] = useState<"matrix" | "quotes">("matrix");

  const tickerLogsCount = useMemo(() => {
    const symKey = sym.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];
    return (executionLogs || []).filter((l) => {
      if (!l.symbol) return false;
      const logKey = l.symbol.toUpperCase();
      return logKey === symKey || logKey.split(/[-_/]/)[0] === baseKey;
    }).length;
  }, [executionLogs, symbol]);

  return (
    <div className="p-4 h-full flex flex-col space-y-4 overflow-hidden text-slate-900 font-sans">
      {/* 1. Strategy Ribbon — parameters left, configurator + lifecycle right
             (same layout as the other strategy pages) */}
      <div className="shrink-0 px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-lg border border-surface-border shadow-2xs select-none">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>

          {/* Symbol Pill */}
          <div className="flex items-center space-x-1.5 bg-surface-raised px-2.5 py-1.5 rounded-md border border-surface-border">
            <span className="font-bold text-slate-900">{hasActiveTicker ? symbol : "N/A"}</span>
            {hasActiveTicker && (
              <span className="text-xs font-semibold px-1.5 py-0.2 bg-blue-50 text-accent rounded-sm border border-blue-200">
                Perp
              </span>
            )}
          </div>

          {/* Delta-neutral gate: ticker refuses new orders until the open
              signal's own legs reconcile. */}
          {hasActiveTicker && positionState?.blocked && (
            <div
              className="flex items-center space-x-1.5 px-2.5 py-1.5 rounded-md border bg-amber-50 text-amber-800 border-amber-300"
              title={`Signal ${positionState.blockingSignalIds.join(", ")} is in-flight or imbalanced — new orders on ${symbol} are refused until its legs confirm delta-neutral. Net filled position: ${positionState.netPositionQty}`}
            >
              <span className="w-1.5 h-1.5 rounded-full bg-amber-500 animate-pulse" />
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
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title="Target profit locked in per round-trip after fees and friction — edit via Configure Strategy"
              >
                <span className="text-slate-400">Target</span>
                <span className="font-bold text-slate-900">{currentStrategyConfig.targetProfitBps.toFixed(1)}</span>
                <span className="text-slate-400">BPS</span>
              </div>

              <div
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title={routeHurdleTitle}
              >
                <span className="text-slate-400">Hurdle</span>
                <span className="font-bold text-slate-900">
                  {routeHurdleBps != null ? routeHurdleBps.toFixed(1) : "—"}
                </span>
                <span className="text-slate-400">BPS</span>
              </div>

              <div
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title={`${windowTitle} · percentile trigger P${currentStrategyConfig.windowPercentile}%`}
              >
                <span className="text-slate-400">Window</span>
                <span className="font-bold text-slate-900">{windowLabel}</span>
                <span className="text-slate-400">P{currentStrategyConfig.windowPercentile}%</span>
              </div>

              <div
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title="Execution Mode"
              >
                <span className="text-slate-400">Mode</span>
                <span className="font-bold text-slate-900">
                  {currentStrategyConfig.executionMode === "DOUBLE_WAY" ? "Double-Way" : "Single-Way"}
                </span>
              </div>

              <div
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title="Grid unit sizing and current accumulated units"
              >
                <span className="text-slate-400">Grid</span>
                <span className="font-bold text-slate-900">
                  {currentStrategyConfig.currentAccumulatedUnits}/{currentStrategyConfig.maxAccumulatedUnits}u
                </span>
                <span className="text-slate-400">({currentStrategyConfig.unitSize} {activeTicker.base})</span>
              </div>

              <div
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title={`Effective Safe Leverage: ${safeLeverage.effectiveLeverage}x | Margin: $${safeLeverage.totalMarginRequiredUsd.toFixed(2)} USD/unit | Buffer: ${safeLeverage.liquidationBufferPct.toFixed(1)}%`}
              >
                <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
                <span className="text-slate-400">Lev</span>
                <span className="font-bold text-slate-900">{safeLeverage.effectiveLeverage}x</span>
                <span className="text-slate-400">${safeLeverage.totalMarginRequiredUsd.toFixed(0)}/u</span>
              </div>
            </>
          )}

          {/* Top-of-Book Mid Price & Spread */}
          {orderbooks.length > 0 && (
            <div className="flex items-center space-x-2 bg-surface-raised px-2.5 py-1.5 rounded-md border border-surface-border">
              <span className="text-slate-400">Mid</span>
              <span className="font-bold text-slate-900">
                {orderbooks[0]?.mid_price && orderbooks[0].mid_price > 0 ? formatCurrencyPrice(orderbooks[0].mid_price) : "—"}
              </span>
              <span className="text-slate-300">|</span>
              <span className="text-slate-400">Book Spread</span>
              <span
                className={`font-bold ${
                  overallBestSpreadBps < 10 ? "text-profit" : "text-slate-700"
                }`}
              >
                {overallBestSpreadBps.toFixed(1)} BPS
              </span>
            </div>
          )}

          {/* Max Dislocation Opportunity Chip */}
          {bestMatrixCell && !bestMatrixCell.isDiagonal && (
            <div
              className={`flex items-center space-x-1.5 px-2.5 py-1.5 rounded-md border cursor-pointer transition ${
                bestMatrixCell.netSpreadBps >= minBps
                  ? "bg-profit-dim text-profit border-profit-border hover:bg-emerald-100"
                  : "bg-surface-raised text-slate-700 border-surface-border hover:bg-slate-100"
              }`}
              onClick={() => setSelectedPair({ rowId: bestMatrixCell.rowId, colId: bestMatrixCell.colId })}
              title={`Target top opportunity: Long ${bestMatrixCell.longVenue} / Short ${bestMatrixCell.shortVenue}`}
            >
              <Zap className="w-3 h-3 text-profit" />
              <span className="font-bold">
                Max Arb: {bestMatrixCell.netSpreadBps > 0 ? "+" : ""}
                {bestMatrixCell.netSpreadBps.toFixed(1)} BPS
              </span>
              <span className="text-xs text-slate-500 font-sans">
                ({bestMatrixCell.longVenue} ➔ {bestMatrixCell.shortVenue})
              </span>
            </div>
          )}

          {/* Ticker Stats: Executions & Net PnL */}
          <div className="flex items-center space-x-1.5 font-mono">
            <span
              className="px-2.5 py-1.5 rounded-md bg-surface-raised border border-surface-border text-slate-600"
              title={hasActiveTicker ? `${tickerStats.count} executions for ${symbol}` : "No connected ticker"}
            >
              {hasActiveTicker ? `${tickerStats.count} tx${tickerStats.count === 1 ? "" : "s"}` : "— txs"}
            </span>
            <span
              className={`px-2.5 py-1.5 rounded-md font-semibold border ${
                hasActiveTicker && tickerStats.pnl > 0
                  ? "text-profit bg-profit-dim border-profit-border"
                  : hasActiveTicker && tickerStats.pnl < 0
                  ? "text-loss bg-loss-dim border-loss-border"
                  : "text-slate-500 bg-surface border-surface-border"
              }`}
              title={hasActiveTicker ? `Strategy Net PnL: ${formatCurrencyPnl(tickerStats.pnl)}` : "No connected ticker"}
            >
              {hasActiveTicker ? formatCurrencyPnl(tickerStats.pnl) : "N/A"}
            </span>
          </div>
        </div>

        {/* Right: Configure Strategy + Bot Lifecycle */}
        <div className="flex items-center space-x-3 shrink-0">
          {/* Configure Strategy Button — universal params, no ticker required */}
          <button
            type="button"
            onClick={handleOpenConfig}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-md border border-accent-line/60 hover:border-accent transition shadow-2xs cursor-pointer"
            title="Configure Single Spread strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
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
            <span className="text-xs font-mono text-red-600 max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
        </div>
      </div>

      {/* 2. Page Body — ticker selector column on the left, strategy content
             scrolls in the right column. */}
      <div className="flex-1 min-h-0 flex gap-4">
        {tickerExplorer && (
          <div className="w-72 shrink-0 min-h-0">{tickerExplorer}</div>
        )}
        <div className="flex-1 min-w-0 overflow-y-auto space-y-4">
          {/* Execution Telemetry — post-trade diagnosis for the selected
              ticker, directly beneath the Strategy Parameters ribbon. */}
          <TickerTelemetryGadget
            symbol={sym}
            logs={executionLogs || []}
          />

          {/* Signal Trend — standalone gadget above the matrix stage. */}
          <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs">
            <SignalTrendGadget
              symbol={sym}
              botWindowSec={windowSeconds}
              hurdleBps={minBps}
            />
          </div>

          {/* 3. Main Stage: Dislocation Matrix + Venue Quotes — one tabbed gadget */}
          <div className="bg-surface rounded-lg border border-surface-border shadow-2xs overflow-hidden">
            {/* Tab switcher — same chrome as the bottom dock */}
            <div className="flex items-stretch border-b border-surface-border bg-slate-50/80 select-none">
              <div className="flex items-stretch divide-x divide-surface-border">
                <button
                  type="button"
                  onClick={() => setStageTab("matrix")}
                  className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                    stageTab === "matrix"
                      ? "text-accent font-bold bg-surface"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
                  }`}
                >
                  <LayoutGrid
                    className={`w-3.5 h-3.5 transition shrink-0 ${
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
                  className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                    stageTab === "quotes"
                      ? "text-accent font-bold bg-surface"
                      : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
                  }`}
                >
                  <Table
                    className={`w-3.5 h-3.5 transition shrink-0 ${
                      stageTab === "quotes" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                    }`}
                  />
                  <span>Venue Quotes</span>
                  <span
                    className={`px-1.5 py-0.2 rounded-sm text-xs font-bold font-mono min-w-[16px] text-center leading-tight transition ${
                      stageTab === "quotes"
                        ? "bg-accent text-white shadow-2xs"
                        : "bg-accent-dim text-accent border border-accent-line/60"
                    }`}
                  >
                    {orderbooks.length}
                  </span>
                  {stageTab === "quotes" && (
                    <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
                  )}
                </button>
              </div>
            </div>

            <div className="p-3.5 space-y-3">
              {stageTab === "matrix" ? (
                <>
          {/* Matrix controls: read-me tooltip, Min BPS chip, quant legend */}
          <div className="flex flex-wrap items-center justify-between gap-2">
            <InfoTooltip
              text={`Row (Y) = LONG (Buy leg) ↓ · Column (X) = SHORT (Sell leg) →. Shows Net BPS after deducting taker fees and closing spread friction, alongside delta (Δ) vs the ${windowLabel} persisted baseline.`}
            />

            <div className="flex items-center space-x-3 text-xs font-mono">
              {/* Min BPS Filter — set via Configure Strategy (Target Profit) */}
              <div
                className="flex items-center space-x-1 bg-surface-raised px-2 py-0.5 rounded-sm border border-surface-border"
                title="Minimum net spread threshold — equals Target Profit, edit via Configure Strategy"
              >
                <span className="text-slate-500 text-xs">Min BPS:</span>
                <span className="font-bold text-slate-900 text-xs">{minBps}</span>
              </div>

              {/* Compact Color Legend */}
              <div className="hidden sm:flex items-center space-x-2 text-xs">
                <div className="flex items-center space-x-1">
                  <span className="w-2 h-2 rounded-sm bg-emerald-100 border border-emerald-400 inline-block" />
                  <span className="text-emerald-800 font-medium">≥ {minBps}</span>
                </div>
                <div className="flex items-center space-x-1">
                  <span className="w-2 h-2 rounded-sm bg-sky-100 border border-sky-300 inline-block" />
                  <span className="text-sky-800">&gt; 0</span>
                </div>
                <div className="flex items-center space-x-1">
                  <span className="w-2 h-2 rounded-sm bg-slate-100 border border-slate-300 inline-block" />
                  <span className="text-slate-500">≤ 0</span>
                </div>
              </div>
            </div>
          </div>

          {/* Matrix Table */}
          <div className="overflow-auto max-h-[500px] rounded-md border border-surface-border">
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
                <table className="w-full border-collapse font-mono text-xs">
                  <thead className="sticky top-0 z-20 bg-surface-raised text-slate-500 text-xs tracking-wider">
                    <tr className="bg-surface-raised border-b border-surface-border">
                      <th className="p-2.5 text-left font-semibold text-slate-500 text-xs tracking-wider border-r border-b border-surface-border min-w-[120px] sticky top-0 left-0 bg-surface-raised z-30">
                        <div className="flex items-center justify-between">
                          <span>LONG ↓</span>
                          <span>SHORT →</span>
                        </div>
                      </th>
                      {venueMatrix.venueLabels.map((colVenue, j) => {
                        const sellPrice = orderbooks[j]?.best_bid ?? 0;
                        return (
                          <th
                            key={colVenue}
                            className="p-2 text-center font-bold text-slate-800 border-r border-b border-surface-border last:border-r-0 min-w-[110px] sticky top-0 bg-surface-raised z-20"
                          >
                            <div className="text-xs text-slate-900">{colVenue}</div>
                            <div className="text-xs font-normal text-profit mt-0.5">
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
                        <tr key={venueMatrix.venues[i]} className="hover:bg-surface-raised/40 transition">
                          {/* Row Header (Long Leg) */}
                          <td className="p-2.5 font-semibold text-slate-900 border-r border-surface-border bg-surface-raised/95 sticky left-0 z-10">
                            <div className="text-xs font-bold text-slate-900">{rowLabel}</div>
                            <div className="text-xs font-normal text-loss mt-0.5">
                              Buy @ {formatCurrencyPrice(buyPrice)}
                            </div>
                          </td>

                          {/* Cells */}
                          {row.map((cell) => {
                            const isSelected =
                              selectedCell?.rowId === cell.rowId && selectedCell?.colId === cell.colId;
                            const visuals = getMatrixCellVisuals(cell, minBps);

                            if (cell.isDiagonal) {
                              return (
                                <td
                                  key={`${cell.rowId}-${cell.colId}`}
                                  className="p-2.5 text-center border-r border-surface-border last:border-r-0 bg-surface-raised/40 select-none"
                                >
                                  <span className="text-slate-400 font-mono text-sm">—</span>
                                </td>
                              );
                            }

                            return (
                              <td
                                key={`${cell.rowId}-${cell.colId}`}
                                onClick={() => setSelectedPair({ rowId: cell.rowId, colId: cell.colId })}
                                className={`p-2 text-center border-r border-surface-border last:border-r-0 cursor-pointer transition select-none ${
                                  visuals.bgClass
                                } ${isSelected ? "ring-2 ring-accent shadow-2xs" : ""}`}
                                title={
                                  cell.costReady
                                    ? `Commission ${cell.roundTripCommissionBps?.toFixed(1)} BPS · entry slippage ${cell.entrySlippageBps?.toFixed(1)} BPS · exit slippage ${cell.exitSlippageBps?.toFixed(1)} BPS · exit crossing ${cell.closingFrictionBps.toFixed(1)} BPS · total friction ${cell.expectedFrictionBps?.toFixed(1)} BPS · required gross ${cell.requiredGrossBps?.toFixed(1)} BPS`
                                    : "Route economics unavailable"
                                }
                              >
                                <div className="flex flex-col items-center justify-center space-y-0.5">
                                  <div className={`text-xs font-mono font-bold ${visuals.textClass}`}>
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
                                    className={`flex items-center space-x-0.5 px-1 py-0.2 rounded-sm text-xs font-mono ${visuals.deltaBadgeBg}`}
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
                          })}
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
          </div>
                </>
              ) : (
                <>

            {/* Consolidated Level 2 Table */}
            <div className="overflow-x-auto rounded-md border border-surface-border">
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
                          {book.venue_name}
                        </td>
                        <td className="py-2 px-2.5 text-profit font-semibold">
                          <span className="flex items-center space-x-1">
                            <span>{formatCurrencyPrice(book.best_bid)}</span>
                            {isBestBid && (
                              <span className="px-1 py-0.2 rounded-sm text-xs font-bold bg-profit-dim text-profit border border-profit-border">
                                Best
                              </span>
                            )}
                          </span>
                        </td>
                        <td className="py-2 px-2.5 text-loss font-semibold">
                          <span className="flex items-center space-x-1">
                            <span>{formatCurrencyPrice(book.best_ask)}</span>
                            {isBestAsk && (
                              <span className="px-1 py-0.2 rounded-sm text-xs font-bold bg-loss-dim text-loss border border-loss-border">
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

            {/* Quick venue dislocation footer — only meaningful with live books */}
            {orderbooks.length > 0 && (
              <div className="text-xs font-mono text-slate-500 flex items-center justify-between pt-1">
                <span>
                  Dislocation:{" "}
                  <strong className={overallBestSpreadBps >= minBps ? "text-profit" : "text-slate-700"}>
                    {overallBestSpreadBps > 0 ? "+" : ""}
                    {overallBestSpreadBps.toFixed(1)} BPS
                  </strong>
                </span>
                <span className="text-slate-400">
                  Buy: {orderbooks.find((b) => b.best_ask === overallBestAsk)?.venue_name || "—"} ➔ Sell:{" "}
                  {orderbooks.find((b) => b.best_bid === overallBestBid)?.venue_name || "—"}
                </span>
              </div>
            )}
                </>
              )}
            </div>
          </div>

      {/* 4. Bottom Docked Tabbed Panel: Ranked Opportunities vs Execution History vs Log Monitor */}
      <div className="bg-surface rounded-lg border border-surface-border shadow-2xs overflow-hidden">
        {/* Tab Switcher with dividers */}
        <div className="flex items-stretch justify-between border-b border-surface-border bg-slate-50/80 select-none">
          <div className="flex items-stretch divide-x divide-surface-border border-r border-surface-border">
            <button
              type="button"
              onClick={() => setBottomTab("opportunities")}
              className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                bottomTab === "opportunities"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <Zap
                className={`w-3.5 h-3.5 transition shrink-0 ${
                  bottomTab === "opportunities" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Ranked Opportunities</span>
              <span
                className={`px-1.5 py-0.2 rounded-sm text-xs font-bold font-mono min-w-[16px] text-center leading-tight transition ${
                  bottomTab === "opportunities"
                    ? "bg-accent text-white shadow-2xs"
                    : "bg-accent-dim text-accent border border-accent-line/60"
                }`}
              >
                {opportunities.length}
              </span>
              {bottomTab === "opportunities" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>

            <button
              type="button"
              onClick={() => setBottomTab("history")}
              className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                bottomTab === "history"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <History
                className={`w-3.5 h-3.5 transition shrink-0 ${
                  bottomTab === "history" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Execution History</span>
              <span
                className={`px-1.5 py-0.2 rounded-sm text-xs font-bold font-mono min-w-[16px] text-center leading-tight transition ${
                  bottomTab === "history"
                    ? "bg-accent text-white shadow-2xs"
                    : "bg-slate-200/80 text-slate-600"
                }`}
              >
                {executionLogs?.length || 0}
              </span>
              {bottomTab === "history" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>

            <button
              type="button"
              onClick={() => setBottomTab("logs")}
              className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                bottomTab === "logs"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <Terminal
                className={`w-3.5 h-3.5 transition shrink-0 ${
                  bottomTab === "logs" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Log Monitor</span>
              <span
                className={`px-1.5 py-0.2 rounded-sm text-xs font-bold font-mono min-w-[16px] text-center leading-tight transition ${
                  bottomTab === "logs"
                    ? "bg-accent text-white shadow-2xs"
                    : "bg-slate-200/80 text-slate-600"
                }`}
              >
                {tickerLogsCount}
              </span>
              {bottomTab === "logs" && (
                <div className="absolute bottom-0 left-0 right-0 h-[2.5px] bg-accent" />
              )}
            </button>
          </div>
        </div>

        {/* Tab Content */}
        <div className="p-3.5">
          {bottomTab === "opportunities" ? (
            opportunities.length === 0 ? (
              <div className="border border-dashed border-surface-border rounded-md">
                <EmptyState plain title="No Opportunity Detected" />
              </div>
            ) : (
              <div className="overflow-x-auto max-h-64 overflow-y-auto">
                <table className="w-full text-left text-xs font-mono">
                  <thead className="sticky top-0 bg-surface-raised border-b border-surface-border text-slate-500 text-xs tracking-wider">
                    <tr>
                      <th className="py-2 px-3">Route (Buy / Sell)</th>
                      <th className="py-2 px-3">Buy Price</th>
                      <th className="py-2 px-3">Sell Price</th>
                      <th className="py-2 px-3">Gross Spread</th>
                      <th className="py-2 px-3">Fees</th>
                      <th className="py-2 px-3">Net Spread</th>
                      <th className="py-2 px-3">Est. Profit</th>
                      <th className="py-2 px-3 text-right">Action</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-surface-border">
                    {opportunities.map((opp) => {
                      const isExec = opp.is_executable;
                      return (
                        <tr
                          key={opp.id}
                          className={`hover:bg-surface-raised transition ${
                            isExec ? "bg-accent-dim/30" : ""
                          }`}
                        >
                          <td className="py-2.5 px-3 font-medium text-slate-900 flex items-center space-x-1.5">
                            <span className="text-slate-800 font-semibold">{opp.buy_venue}</span>
                            <span className="text-slate-400 text-xs">➔</span>
                            <span className="text-slate-800 font-semibold">{opp.sell_venue}</span>
                          </td>
                          <td className="py-2.5 px-3 text-loss font-medium">
                            {formatCurrencyPrice(opp.buy_price)}
                          </td>
                          <td className="py-2.5 px-3 text-profit font-medium">
                            {formatCurrencyPrice(opp.sell_price)}
                          </td>
                          <td className="py-2.5 px-3 text-slate-700">
                            {opp.gross_spread_bps.toFixed(1)} BPS
                          </td>
                          <td className="py-2.5 px-3 text-slate-500">
                            {opp.fee_bps.toFixed(1)} BPS
                          </td>
                          <td className="py-2.5 px-3">
                            <span
                              className={`font-bold ${
                                opp.net_spread_bps >= minBps ? "text-profit" : "text-slate-500"
                              }`}
                            >
                              {opp.net_spread_bps.toFixed(2)} BPS
                            </span>
                          </td>
                          <td
                            className={`py-2.5 px-3 font-bold ${
                              opp.estimated_profit_usd >= 0 ? "text-profit" : "text-loss"
                            }`}
                          >
                            {formatCurrencyPnl(opp.estimated_profit_usd)}
                          </td>
                          <td className="py-2.5 px-3 text-right">
                            <button
                              onClick={() => {
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
                              }}
                              disabled={
                                !isExec ||
                                loading ||
                                positionState?.blocked ||
                                (bot.phase !== "idle" && bot.phase !== "running")
                              }
                              title={
                                positionState?.blocked
                                  ? `${symbol} blocked: signal ${positionState.blockingSignalIds.join(", ")} awaiting delta-neutral reconciliation`
                                  : bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping"
                                  ? `Entries blocked: bot is ${bot.phase.replace("_", "-")}`
                                  : undefined
                              }
                              className={`px-3 py-1.5 rounded-sm text-xs font-mono font-medium transition shadow-2xs ${
                                isExec &&
                                !positionState?.blocked &&
                                (bot.phase === "idle" || bot.phase === "running")
                                  ? "bg-accent hover:bg-accent-hover text-white"
                                  : "bg-surface-raised text-slate-400 cursor-not-allowed border border-surface-border"
                              }`}
                            >
                              Execute
                            </button>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )
          ) : bottomTab === "history" ? (
            <div className="max-h-96 overflow-y-auto">
              <TickerTransactionHistory
                symbol={sym}
                logs={executionLogs || []}
                config={currentStrategyConfig}
                onRefresh={() => onRefresh(orderQty, minBps)}
              />
            </div>
          ) : (
            <div className="overflow-y-auto">
              <TickerLogMonitor
                symbol={sym}
                strategyName="Single Spread"
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
          onClose={() => setInternalConfigModalOpen(false)}
          tickerSymbol={sym}
          onSaveConfig={handleSaveStrategyParams}
          initialStrategy="futures"
        />
      )}
    </div>
  );
};
