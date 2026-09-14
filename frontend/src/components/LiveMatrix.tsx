import React, { useState, useEffect, useMemo } from "react";
import {
  TrendingUp,
  TrendingDown,
  Zap,
  History,
  Terminal,
  Activity,
  SlidersHorizontal,
  ShieldCheck,
  LineChart,
} from "lucide-react";
import type {
  VenueOrderBookDto,
  SpatialOpportunityDto,
  ExecutionLogEntry,
  SlidingWindowOption,
  MatrixCellDto,
  TickerStrategyConfigDto,
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
  WINDOW_SECONDS,
  buildCrossVenueMatrix,
  getMatrixCellVisuals,
  globalSpreadTracker,
} from "../services/matrixQuant";
import { getMinCompatibleSize, computeSafeLeverage } from "../services/contractSpecs";
import { InfoTooltip } from "./InfoTooltip";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { useBotLifecycle } from "../hooks/useBotLifecycle";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerTelemetryGadget } from "./TickerTelemetryGadget";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { SignalTrendGadget } from "./SignalTrendGadget";
import { TickerConfigModal } from "./TickerConfigModal";
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
  const [minBps, setMinBps] = useState(5.0);
  const [selectedPair, setSelectedPair] = useState<{ rowId: string; colId: string } | null>(null);
  const [executing, setExecuting] = useState(false);
  const [internalConfigModalOpen, setInternalConfigModalOpen] = useState(false);
  const lastAutoExecTimeRef = React.useRef<number>(0);

  // Per-ticker quantitative strategy configurations
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(() => new Map());

  const currentStrategyConfig: TickerStrategyConfigDto = useMemo(() => {
    const existing = strategyConfigs.get(sym);
    if (existing) return existing;

    const minLot = getMinCompatibleSize(sym, activeTicker.venues, activeTicker.basePrice).minCompatibleSize;
    return {
      symbol: sym,
      targetProfitBps: minBps,
      windowPercentile: 50,
      maxAccumulatedUnits: 3,
      currentAccumulatedUnits: 0,
      executionMode: "SINGLE_WAY",
      unitSize: minLot,
      userTargetLeverage: 5,
      slidingWindow: "1m",
    };
  }, [symbol, strategyConfigs, activeTicker.venues, activeTicker.basePrice, minBps]);

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

  // Rolling Sliding Window is driven by strategy config
  const windowOption: SlidingWindowOption = useMemo(() => {
    return (currentStrategyConfig.slidingWindow || currentStrategyConfig.sliding_window || "1m") as SlidingWindowOption;
  }, [currentStrategyConfig]);

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

  const handleUpdateStrategyConfig = (newCfg: TickerStrategyConfigDto) => {
    setStrategyConfigs((prev) => {
      const next = new Map(prev);
      next.set(newCfg.symbol, newCfg);
      return next;
    });
    setMinBps(newCfg.targetProfitBps);
    setOrderQty(newCfg.unitSize);
    api.saveTickerStrategyConfig(newCfg).catch((err) => {
      console.warn("Failed to persist ticker strategy config:", err);
    });
  };

  // Load persisted strategy config for active symbol on select
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
          setMinBps(cfg.targetProfitBps);
          setOrderQty(cfg.unitSize);
        }
      })
      .catch((err) => {
        console.warn("Failed to fetch ticker strategy config:", err);
      });
    return () => {
      isMounted = false;
    };
  }, [symbol]);

  // Sync benchmark order quantity when active asset changes
  useEffect(() => {
    setOrderQty(getDefaultOrderQty(activeTicker.basePrice));
  }, [symbol, activeTicker.basePrice]);

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

  const windowSeconds = WINDOW_SECONDS[windowOption] ?? 60;

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
      globalSpreadTracker
    );
  }, [symbol, orderbooks, windowSeconds, orderQty, minBps, currentStrategyConfig]);

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
      const opp: SpatialOpportunityDto = {
        id: `${cell.longSymbol}-${cell.rowId}-${cell.colId}`,
        symbol: cell.longSymbol,
        buy_venue: cell.longVenue,
        sell_venue: cell.shortVenue,
        buy_price: cell.longPrice,
        sell_price: cell.shortPrice,
        gross_spread: cell.shortPrice - cell.longPrice,
        gross_spread_bps: cell.grossSpreadBps,
        fee_bps: cell.feeBps + cell.closingFrictionBps,
        net_spread_bps: cell.netSpreadBps,
        order_qty: cell.orderQty,
        estimated_profit_usd: cell.estProfitUsd,
        is_executable: cell.isExecutable,
        direction: cell.isConvergenceUnwind ? "Convergence Unwind" : "Divergence Entry",
        trigger_spread_bps: cell.netSpreadBps,
        baseline_mean_spread_bps: cell.baselineMeanBps,
        position_step: cell.isConvergenceUnwind
          ? currentStrategyConfig.currentAccumulatedUnits
          : currentStrategyConfig.currentAccumulatedUnits + 1,
        max_units: currentStrategyConfig.maxAccumulatedUnits,
        nominal_hurdle_bps: nominalHurdleBps,
      };

      await onExecuteTrade(opp);

      // Track accumulated grid steps
      if (cell.isConvergenceUnwind) {
        handleUpdateStrategyConfig({
          ...currentStrategyConfig,
          currentAccumulatedUnits: Math.max(0, currentStrategyConfig.currentAccumulatedUnits - 1),
        });
      } else {
        handleUpdateStrategyConfig({
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

  const nominalHurdleBps = useMemo(() => {
    return Math.round((7.0 + 0.6 + currentStrategyConfig.targetProfitBps) * 100) / 100;
  }, [currentStrategyConfig.targetProfitBps]);

  const [bottomTab, setBottomTab] = useState<"opportunities" | "signal" | "history" | "telemetry" | "logs">("opportunities");

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
    <div className="p-4 space-y-4 overflow-y-auto h-full text-slate-900 font-sans">
      {/* 1. Strategy Ribbon — parameters left, configurator + lifecycle right
             (same layout as the other strategy pages) */}
      <div className="px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs select-none">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-xs font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>

          {/* Symbol Pill */}
          <div className="flex items-center space-x-1.5 bg-surface-raised px-2.5 py-1.5 rounded-md border border-surface-border">
            <span className="font-bold text-slate-900">{hasActiveTicker ? symbol : "N/A"}</span>
            {hasActiveTicker && (
              <span className="text-xs font-semibold px-1.5 py-0.2 bg-blue-50 text-accent rounded-sm border border-blue-200">
                PERP
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
                title="Nominal hurdle including fees and friction"
              >
                <span className="text-slate-400">Hurdle</span>
                <span className="font-bold text-slate-900">{nominalHurdleBps.toFixed(1)}</span>
                <span className="text-slate-400">BPS</span>
              </div>

              <div
                className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
                title={`Rolling ${windowOption} window percentile trigger (P${currentStrategyConfig.windowPercentile}%)`}
              >
                <span className="text-slate-400">Window</span>
                <span className="font-bold text-slate-900">{windowOption}</span>
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
          {/* Configure Strategy Button */}
          <button
            type="button"
            onClick={handleOpenConfig}
            disabled={!hasActiveTicker}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-md border border-accent-line/60 hover:border-accent transition shadow-xs cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
            title={hasActiveTicker ? `Configure ${symbol} quantitative parameters in modal` : "Connect at least 2 venues to select a ticker"}
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

      {/* 3. Main Stage: Dual-Split Layout (Left: 2D Matrix Grid, Right: Consolidated Quotes + Instant Dock) */}
      <div className="grid grid-cols-12 gap-4 items-start">
        {/* Left: 2D Interactive Arbitrage Matrix */}
        <div className="col-span-12 xl:col-span-7 bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-3">
          {/* Header with Title, Quick Min BPS, and Quant Legend */}
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-surface-border pb-2.5">
            <div>
              <h3 className="text-xs font-bold font-mono tracking-wider text-slate-900 flex items-center space-x-2">
                <span>Arbitrage Dislocation Matrix</span>
                <InfoTooltip
                  text={`Row (Y) = LONG (Buy leg) ↓ · Column (X) = SHORT (Sell leg) →. Shows Net BPS after deducting taker fees and closing spread friction, alongside delta (Δ) vs rolling ${windowOption} baseline.`}
                />
              </h3>
            </div>

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
              <div className="py-12 text-center text-slate-500 font-mono text-xs px-4 space-y-1.5">
                  {symbol === null ? (
                    <>
                      <div className="font-bold text-slate-800 text-sm">No Ticker Selected</div>
                      <div className="text-xs text-slate-500">
                        Pick a market in the explorer to stream its live venue quotes.
                      </div>
                    </>
                  ) : (
                    <>
                      <div className="font-bold text-slate-800 text-sm">Cross-Venue Arbitrage Unavailable</div>
                      <div className="text-xs text-slate-500">
                        At least 2 enabled &amp; connected exchanges are required for this ticker.
                      </div>
                    </>
                  )}
                </div>
              ) : (
                <table className="w-full border-collapse font-mono text-xs">
                  <thead className="sticky top-0 z-20 bg-surface-raised shadow-2xs">
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
                                } ${isSelected ? "ring-2 ring-accent shadow-xs" : ""}`}
                                title={`Click to target: Long ${cell.rowLabel} / Short ${cell.colLabel}`}
                              >
                                <div className="flex flex-col items-center justify-center space-y-0.5">
                                  <div className={`text-xs font-mono font-bold ${visuals.textClass}`}>
                                    {cell.netSpreadBps > 0 ? "+" : ""}
                                    {cell.netSpreadBps.toFixed(1)} BPS
                                  </div>

                                  <div
                                    className={`flex items-center space-x-0.5 px-1 py-0.2 rounded-sm text-xs font-mono ${visuals.deltaBadgeBg}`}
                                  >
                                    {cell.deltaBps > 0 ? (
                                      <TrendingUp className="w-2 h-2 inline" />
                                    ) : cell.deltaBps < 0 ? (
                                      <TrendingDown className="w-2 h-2 inline" />
                                    ) : null}
                                    <span>
                                      Δ {cell.deltaBps > 0 ? "+" : ""}
                                      {cell.deltaBps.toFixed(1)}
                                    </span>
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
        </div>

        {/* Right: Consolidated Level 2 Quotes + Instant Atomic Execution Dock */}
        <div className="col-span-12 xl:col-span-5 space-y-4">
          {/* Widget 1: Consolidated Level 2 Venue Quote Board */}
          <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-2.5">
            <div className="flex items-center justify-between border-b border-surface-border pb-2">
              <div className="flex items-center space-x-2">
                <span className="font-mono text-xs font-bold tracking-wider text-slate-900">
                  Consolidated Venue Quotes
                </span>
              </div>
              <span className="text-xs font-mono text-slate-400">
                {orderbooks.length} Venues Active
              </span>
            </div>

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
                      <td colSpan={5} className="py-8 px-2.5 text-center text-slate-400 font-mono text-xs">
                        {symbol === null
                          ? "No ticker selected — venue quotes appear here once a market is picked."
                          : "No venue quotes — enable and connect at least one exchange."}
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
                                BEST
                              </span>
                            )}
                          </span>
                        </td>
                        <td className="py-2 px-2.5 text-loss font-semibold">
                          <span className="flex items-center space-x-1">
                            <span>{formatCurrencyPrice(book.best_ask)}</span>
                            {isBestAsk && (
                              <span className="px-1 py-0.2 rounded-sm text-xs font-bold bg-loss-dim text-loss border border-loss-border">
                                BEST
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
          </div>
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
              onClick={() => setBottomTab("signal")}
              className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                bottomTab === "signal"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <LineChart
                className={`w-3.5 h-3.5 transition shrink-0 ${
                  bottomTab === "signal" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Signal Trend</span>
              {bottomTab === "signal" && (
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
              onClick={() => setBottomTab("telemetry")}
              className={`relative h-10 flex items-center space-x-2 px-4 text-xs font-mono transition select-none group cursor-pointer ${
                bottomTab === "telemetry"
                  ? "text-accent font-bold bg-surface"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/70 font-medium"
              }`}
            >
              <Activity
                className={`w-3.5 h-3.5 transition shrink-0 ${
                  bottomTab === "telemetry" ? "text-accent" : "text-slate-400 group-hover:text-slate-600"
                }`}
              />
              <span>Telemetry</span>
              {bottomTab === "telemetry" && (
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
              <div className="text-center py-6 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-md">
                No spatial price discrepancies currently exceeding {minBps} BPS threshold.
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
                                  nominal_hurdle_bps: currentStrategyConfig.targetProfitBps + 3.5,
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
                              className={`px-3 py-1 rounded-sm text-xs font-mono font-medium transition shadow-2xs ${
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
          ) : bottomTab === "signal" ? (
            <div className="max-h-96 overflow-y-auto">
              <SignalTrendGadget
                symbol={sym}
                botWindowSec={windowSeconds}
                hurdleBps={minBps}
              />
            </div>
          ) : bottomTab === "history" ? (
            <div className="max-h-96 overflow-y-auto">
              <TickerTransactionHistory
                symbol={sym}
                logs={executionLogs || []}
                config={currentStrategyConfig}
                onRefresh={() => onRefresh(orderQty, minBps)}
              />
            </div>
          ) : bottomTab === "telemetry" ? (
            <div className="max-h-96 overflow-y-auto">
              <TickerTelemetryGadget
                symbol={sym}
                logs={executionLogs || []}
                config={currentStrategyConfig}
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

      {/* Fallback Internal Config Modal */}
      {!onOpenTickerConfig && (
        <TickerConfigModal
          isOpen={internalConfigModalOpen}
          onClose={() => setInternalConfigModalOpen(false)}
          tickerSymbol={sym}
          strategyConfig={currentStrategyConfig}
          onSaveConfig={handleUpdateStrategyConfig}
          initialStrategy="futures"
        />
      )}
    </div>
  );
};
