import React, { useState, useEffect, useMemo } from "react";
import {
  TrendingUp,
  TrendingDown,
  Zap,
  History,
  Terminal,
  SlidersHorizontal,
  ShieldCheck,
} from "lucide-react";
import type {
  VenueOrderBookDto,
  SpatialOpportunityDto,
  ExecutionLogEntry,
  SlidingWindowOption,
  MatrixCellDto,
  TickerStrategyConfigDto,
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
import { SwitchToggle } from "./SwitchToggle";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerLogMonitor } from "./TickerLogMonitor";
import { TickerConfigModal } from "./TickerConfigModal";
import { api } from "../services/api";

interface LiveMatrixProps {
  symbol: string;
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
}) => {
  const rawTicker = (availableTickers ?? PERP_TICKERS).find((t) => t.symbol === symbol) || {
    symbol,
    base: symbol.split("-")[0],
    quote: symbol.split("-")[1] || "USDC",
    name: symbol,
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

  // State
  const [orderQty, setOrderQty] = useState(() => getDefaultOrderQty(activeTicker.basePrice));
  const [minBps, setMinBps] = useState(5.0);
  const [selectedPair, setSelectedPair] = useState<{ rowId: string; colId: string } | null>(null);
  const [executing, setExecuting] = useState(false);
  const [autoExecute, setAutoExecute] = useState(false);
  const [internalConfigModalOpen, setInternalConfigModalOpen] = useState(false);
  const lastAutoExecTimeRef = React.useRef<number>(0);

  const handleToggleAutoExecute = (enabled: boolean) => {
    setAutoExecute(enabled);
    if (enabled && !isTickerEnabled && onToggleTicker) {
      onToggleTicker(symbol, true);
    }
  };

  // Per-ticker quantitative strategy configurations
  const [strategyConfigs, setStrategyConfigs] = useState<Map<string, TickerStrategyConfigDto>>(() => new Map());

  const currentStrategyConfig: TickerStrategyConfigDto = useMemo(() => {
    const existing = strategyConfigs.get(symbol);
    if (existing) return existing;

    const minLot = getMinCompatibleSize(symbol, activeTicker.venues, activeTicker.basePrice).minCompatibleSize;
    return {
      symbol,
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
      onOpenTickerConfig(symbol, "futures");
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
    let isMounted = true;
    api
      .getTickerStrategyConfig(symbol)
      .then((cfg) => {
        if (isMounted && cfg) {
          setStrategyConfigs((prev) => {
            const next = new Map(prev);
            next.set(symbol, cfg);
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
    const symKey = symbol.toUpperCase();
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
      symbol,
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

  // Automated Execution Engine Loop for Futures Spread Convergence
  useEffect(() => {
    if (!autoExecute || executing || !isTickerEnabled || isPaused) return;

    const now = Date.now();
    if (now - lastAutoExecTimeRef.current < 4000) return; // 4s cooldown between auto-trades

    let bestCell: MatrixCellDto | null = null;
    let highestNet = -Infinity;

    for (const row of venueMatrix.grid) {
      for (const cell of row) {
        if (cell.isDiagonal) continue;
        if (cell.isExecutable && cell.netSpreadBps > highestNet) {
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
  }, [autoExecute, executing, venueMatrix.grid, currentStrategyConfig, isTickerEnabled, isPaused]);

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

  const [bottomTab, setBottomTab] = useState<"opportunities" | "history" | "logs">("opportunities");

  const tickerLogsCount = useMemo(() => {
    const symKey = symbol.toUpperCase();
    const baseKey = symKey.split(/[-_/]/)[0];
    return (executionLogs || []).filter((l) => {
      if (!l.symbol) return false;
      const logKey = l.symbol.toUpperCase();
      return logKey === symKey || logKey.split(/[-_/]/)[0] === baseKey;
    }).length;
  }, [executionLogs, symbol]);

  return (
    <div className="p-3.5 space-y-3 overflow-y-auto h-full text-slate-900 font-sans">
      {/* 1. Unified Ticker & Strategy Header Ribbon */}
      <div className="bg-surface rounded-xl border border-surface-border p-2.5 shadow-2xs flex flex-wrap items-center justify-between gap-2.5 text-xs font-mono select-none">
        {/* Left: Ticker identity, top price, max dislocation, executions & PnL, strategy parameters */}
        <div className="flex flex-wrap items-center gap-2">
          {/* Symbol Pill */}
          <div className="flex items-center space-x-1.5 bg-surface-raised px-2.5 py-1 rounded-lg border border-surface-border">
            <span className="font-bold text-slate-900 text-sm">{symbol}</span>
            <span className="text-[10px] font-semibold px-1.5 py-0.2 bg-blue-50 text-accent rounded border border-blue-200">
              PERP
            </span>
          </div>

          {/* Top-of-Book Mid Price & Spread */}
          {orderbooks.length > 0 && (
            <div className="flex items-center space-x-2 bg-surface-raised px-2.5 py-1 rounded-lg border border-surface-border">
              <span className="text-slate-400 text-[11px]">Mid:</span>
              <span className="font-bold text-slate-900">
                {orderbooks[0]?.mid_price && orderbooks[0].mid_price > 0 ? formatCurrencyPrice(orderbooks[0].mid_price) : "—"}
              </span>
              <span className="text-slate-300">|</span>
              <span className="text-slate-400 text-[11px]">Book Spread:</span>
              <span
                className={`font-semibold ${
                  overallBestSpreadBps < 10 ? "text-profit" : "text-slate-700"
                }`}
              >
                {overallBestSpreadBps.toFixed(1)} bps
              </span>
            </div>
          )}

          {/* Max Dislocation Opportunity Chip */}
          {bestMatrixCell && !bestMatrixCell.isDiagonal && (
            <div
              className={`flex items-center space-x-1.5 px-2.5 py-1 rounded-lg border cursor-pointer transition ${
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
              <span className="text-[10px] text-slate-500 font-sans">
                ({bestMatrixCell.longVenue} ➔ {bestMatrixCell.shortVenue})
              </span>
            </div>
          )}

          {/* Ticker Stats: Executions & Net PnL */}
          <div className="flex items-center space-x-1.5 text-[11px] font-mono">
            <span
              className="px-1.5 py-0.5 rounded bg-surface-raised border border-surface-border text-slate-600"
              title={`${tickerStats.count} executions for ${symbol}`}
            >
              {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
            </span>
            <span
              className={`px-1.5 py-0.5 rounded font-semibold ${
                tickerStats.pnl > 0
                  ? "text-profit bg-profit-dim border border-profit-border"
                  : tickerStats.pnl < 0
                  ? "text-loss bg-loss-dim border border-loss-border"
                  : "text-slate-500 bg-surface border border-surface-border"
              }`}
              title={`Strategy Net PnL: ${formatCurrencyPnl(tickerStats.pnl)}`}
            >
              {formatCurrencyPnl(tickerStats.pnl)}
            </span>
          </div>

          {/* Unified Strategy Parameters (Single Non-Duplicate Hurdle) */}
          <div className="hidden md:flex items-center space-x-1.5 text-[11px] font-mono">
            <span
              className="bg-blue-50 text-blue-700 px-2 py-0.5 rounded border border-blue-200/60 font-semibold"
              title="Target profit locked in per round-trip after fees and friction"
            >
              TP {currentStrategyConfig.targetProfitBps.toFixed(1)} bps
            </span>

            <span
              className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded border border-surface-border"
              title={`Nominal Hurdle: ${nominalHurdleBps.toFixed(1)} bps`}
            >
              Hurdle <strong className="text-slate-900 font-semibold">{nominalHurdleBps.toFixed(1)} bps</strong>
            </span>

            <span
              className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded border border-surface-border"
              title={`Rolling ${windowOption} window percentile trigger (P${currentStrategyConfig.windowPercentile}%)`}
            >
              Window {windowOption} (P{currentStrategyConfig.windowPercentile}%)
            </span>

            <span
              className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded border border-surface-border"
              title="Execution Mode"
            >
              {currentStrategyConfig.executionMode === "DOUBLE_WAY" ? "Double-Way" : "Single-Way"}
            </span>

            <span
              className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded border border-surface-border"
              title="Grid unit sizing and current accumulated units"
            >
              Grid <strong className="text-slate-900">{currentStrategyConfig.currentAccumulatedUnits}/{currentStrategyConfig.maxAccumulatedUnits}u</strong> ({currentStrategyConfig.unitSize} {activeTicker.base})
            </span>

            <span
              className="bg-surface-raised text-slate-700 px-2 py-0.5 rounded border border-surface-border flex items-center space-x-1"
              title={`Effective Safe Leverage: ${safeLeverage.effectiveLeverage}x | Margin: $${safeLeverage.totalMarginRequiredUsd.toFixed(2)} USD/unit | Buffer: ${safeLeverage.liquidationBufferPct.toFixed(1)}%`}
            >
              <ShieldCheck className="w-3 h-3 text-emerald-600" />
              <span>Lev {safeLeverage.effectiveLeverage}x</span>
              <span className="text-slate-300">|</span>
              <span className="text-slate-600">${safeLeverage.totalMarginRequiredUsd.toFixed(0)}/u</span>
            </span>
          </div>
        </div>

        {/* Right: Auto-Bot and Configure Strategy Button */}
        <div className="flex items-center space-x-2.5 shrink-0">
          {/* Auto-Bot Switch Toggle (Green for ON, White for OFF) */}
          <div className="flex items-center space-x-1.5 px-2 py-0.5 bg-surface border border-surface-border rounded-lg shadow-2xs">
            <span className="text-xs font-mono font-medium text-slate-700">Auto Bot</span>
            <SwitchToggle
              checked={autoExecute}
              onChange={handleToggleAutoExecute}
              size="sm"
              showStatusText={true}
            />
          </div>

          {/* Configure Strategy Button */}
          <button
            type="button"
            onClick={handleOpenConfig}
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface hover:bg-slate-100 text-accent font-semibold text-xs font-mono rounded-lg border border-accent-line/60 hover:border-accent transition shadow-xs cursor-pointer"
            title={`Configure ${symbol} quantitative parameters in modal`}
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
          </button>
        </div>
      </div>

      {/* 3. Main Stage: Dual-Split Layout (Left: 2D Matrix Grid, Right: Consolidated Quotes + Instant Dock) */}
      <div className="grid grid-cols-12 gap-3 items-start">
        {/* Left: 2D Interactive Arbitrage Matrix */}
        <div className="col-span-12 xl:col-span-7 bg-surface rounded-xl border border-surface-border p-3.5 shadow-2xs space-y-3">
          {/* Header with Title, Quick Min BPS, and Quant Legend */}
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-surface-border pb-2.5">
            <div>
              <h3 className="text-xs font-bold font-mono text-slate-900 flex items-center space-x-2">
                <span>Arbitrage Dislocation Matrix — [{symbol}]</span>
                <span className="px-1.5 py-0.2 rounded text-[10px] bg-accent-dim text-accent border border-accent-line font-medium">
                  Rolling {windowOption} Window
                </span>
                <InfoTooltip
                  text={`Row (Y) = LONG (Buy leg) ↓ · Column (X) = SHORT (Sell leg) →. Shows Net BPS after deducting taker fees and closing spread friction, alongside delta (Δ) vs rolling ${windowOption} baseline.`}
                />
              </h3>
            </div>

            <div className="flex items-center space-x-3 text-[11px] font-mono">
              {/* Min BPS Filter */}
              <div className="flex items-center space-x-1 bg-surface-raised px-2 py-0.5 rounded border border-surface-border">
                <span className="text-slate-500 text-[10px]">Min BPS:</span>
                <input
                  type="number"
                  step="0.5"
                  value={minBps}
                  onChange={(e) => setMinBps(parseFloat(e.target.value) || 0)}
                  className="w-12 px-1 py-0.2 bg-surface border border-surface-border rounded text-slate-900 font-mono text-[11px] text-center focus:outline-none focus:ring-1 focus:ring-accent"
                />
              </div>

              {/* Compact Color Legend */}
              <div className="hidden sm:flex items-center space-x-2 text-[10px]">
                <div className="flex items-center space-x-1">
                  <span className="w-2 h-2 rounded bg-emerald-100 border border-emerald-400 inline-block" />
                  <span className="text-emerald-800 font-medium">≥ {minBps}</span>
                </div>
                <div className="flex items-center space-x-1">
                  <span className="w-2 h-2 rounded bg-sky-100 border border-sky-300 inline-block" />
                  <span className="text-sky-800">&gt; 0</span>
                </div>
                <div className="flex items-center space-x-1">
                  <span className="w-2 h-2 rounded bg-slate-100 border border-slate-300 inline-block" />
                  <span className="text-slate-500">≤ 0</span>
                </div>
              </div>
            </div>
          </div>

          {/* Matrix Table */}
          <div className="overflow-auto max-h-[500px] rounded-lg border border-surface-border">
            {venueMatrix.grid.length < 2 ? (
              <div className="py-12 text-center text-slate-500 font-mono text-xs px-4 space-y-1.5">
                  <div className="font-bold text-slate-800 text-sm">Cross-Venue Arbitrage Unavailable</div>
                  <div className="text-[11px] text-slate-500">
                    At least 2 enabled &amp; connected exchanges are required for this ticker.
                  </div>
                </div>
              ) : (
                <table className="w-full border-collapse font-mono text-xs">
                  <thead className="sticky top-0 z-20 bg-surface-raised shadow-2xs">
                    <tr className="bg-surface-raised border-b border-surface-border">
                      <th className="p-2.5 text-left font-semibold text-slate-500 uppercase text-[10px] tracking-wider border-r border-b border-surface-border min-w-[120px] sticky top-0 left-0 bg-surface-raised z-30">
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
                            <div className="text-[11px] text-slate-900">{colVenue}</div>
                            <div className="text-[10px] font-normal text-profit mt-0.5">
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
                            <div className="text-[11px] font-bold text-slate-900">{rowLabel}</div>
                            <div className="text-[10px] font-normal text-loss mt-0.5">
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
                                    className={`flex items-center space-x-0.5 px-1 py-0.2 rounded text-[9px] font-mono ${visuals.deltaBadgeBg}`}
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
        <div className="col-span-12 xl:col-span-5 space-y-3">
          {/* Widget 1: Consolidated Level 2 Venue Quote Board */}
          <div className="bg-surface rounded-xl border border-surface-border p-3.5 shadow-2xs space-y-2.5">
            <div className="flex items-center justify-between border-b border-surface-border pb-2">
              <div className="flex items-center space-x-2">
                <span className="font-mono text-xs font-bold text-slate-900">
                  Consolidated Venue Quotes
                </span>
                <span className="px-1.5 py-0.2 rounded text-[10px] font-mono bg-surface-raised border border-surface-border text-slate-600">
                  Level 2 Top-of-Book
                </span>
              </div>
              <span className="text-[10px] font-mono text-slate-400">
                {orderbooks.length} Venues Active
              </span>
            </div>

            {/* Consolidated Level 2 Table */}
            <div className="overflow-x-auto rounded-lg border border-surface-border">
              <table className="w-full text-left font-mono text-xs">
                <thead className="bg-surface-raised text-[10px] text-slate-500 uppercase tracking-wider border-b border-surface-border">
                  <tr>
                    <th className="py-2 px-2.5">Venue</th>
                    <th className="py-2 px-2.5">Bid (Sell)</th>
                    <th className="py-2 px-2.5">Ask (Buy)</th>
                    <th className="py-2 px-2 text-right">Spread</th>
                    <th className="py-2 px-2.5 text-right">8h Fund</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border text-[11px]">
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
                              <span className="px-1 py-0.2 rounded text-[9px] font-bold bg-profit-dim text-profit border border-profit-border">
                                BEST
                              </span>
                            )}
                          </span>
                        </td>
                        <td className="py-2 px-2.5 text-loss font-semibold">
                          <span className="flex items-center space-x-1">
                            <span>{formatCurrencyPrice(book.best_ask)}</span>
                            {isBestAsk && (
                              <span className="px-1 py-0.2 rounded text-[9px] font-bold bg-loss-dim text-loss border border-loss-border">
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

            {/* Quick venue dislocation footer */}
            <div className="text-[11px] font-mono text-slate-500 flex items-center justify-between pt-1">
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
          </div>
        </div>
      </div>

      {/* 4. Bottom Docked Tabbed Panel: Ranked Opportunities vs Execution History vs Log Monitor */}
      <div className="bg-surface rounded-xl border border-surface-border shadow-2xs overflow-hidden">
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
                className={`px-1.5 py-0.2 rounded text-[10px] font-bold font-mono min-w-[16px] text-center leading-tight transition ${
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
              <span>Execution History & Telemetry</span>
              <span
                className={`px-1.5 py-0.2 rounded text-[10px] font-bold font-mono min-w-[16px] text-center leading-tight transition ${
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
                className={`px-1.5 py-0.2 rounded text-[10px] font-bold font-mono min-w-[16px] text-center leading-tight transition ${
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
              <div className="text-center py-6 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-lg">
                No spatial price discrepancies currently exceeding {minBps} BPS threshold.
              </div>
            ) : (
              <div className="overflow-x-auto max-h-64 overflow-y-auto">
                <table className="w-full text-left text-xs font-mono">
                  <thead className="sticky top-0 bg-surface-raised border-b border-surface-border text-slate-500 uppercase text-[10px] tracking-wider">
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
                              disabled={!isExec || loading}
                              className={`px-3 py-1 rounded text-xs font-mono font-medium transition shadow-2xs ${
                                isExec
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
                symbol={symbol}
                logs={executionLogs || []}
                config={currentStrategyConfig}
                onRefresh={() => onRefresh(orderQty, minBps)}
              />
            </div>
          ) : (
            <div className="overflow-y-auto">
              <TickerLogMonitor
                symbol={symbol}
                strategyName="Spread Range"
                config={currentStrategyConfig}
                executionLogs={executionLogs || []}
                orderbooks={orderbooks}
                autoBotEnabled={autoExecute}
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
          tickerSymbol={symbol}
          strategyConfig={currentStrategyConfig}
          onSaveConfig={handleUpdateStrategyConfig}
          initialStrategy="futures"
        />
      )}
    </div>
  );
};
