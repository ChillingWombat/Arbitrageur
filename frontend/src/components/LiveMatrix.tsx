import React, { useState, useEffect, useMemo } from "react";
import {
  RefreshCw,
  TrendingUp,
  TrendingDown,
  Layers,
  Grid3X3,
  Zap,
  CheckCircle2,
  ArrowRight,
  SlidersHorizontal,
  History,
} from "lucide-react";
import type {
  VenueOrderBookDto,
  SpatialOpportunityDto,
  ExecutionLogEntry,
  MatrixMode,
  SlidingWindowOption,
  MatrixCellDto,
  CrossTickerExecutionParams,
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
  buildCrossTickerMatrix,
  getMatrixCellVisuals,
  globalSpreadTracker,
  type TickerMarketQuote,
} from "../services/matrixQuant";
import { getMinCompatibleSize, computeSafeLeverage } from "../services/contractSpecs";
import { TickerStrategyBar } from "./TickerStrategyBar";
import { InfoTooltip } from "./InfoTooltip";
import { SwitchToggle } from "./SwitchToggle";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { api } from "../services/api";

interface LiveMatrixProps {
  symbol: string;
  orderbooks: VenueOrderBookDto[];
  opportunities: SpatialOpportunityDto[];
  onRefresh: (qty?: number, minBps?: number) => void;
  onExecuteTrade: (opp: SpatialOpportunityDto) => Promise<void>;
  onExecuteCrossTicker?: (params: CrossTickerExecutionParams) => Promise<void>;
  loading: boolean;
  executionLogs?: ExecutionLogEntry[];
  onOpenTickerConfig?: (symbol: string, strategy?: string) => void;
  activeVenueCodes?: Set<string>;
  availableTickers?: PerpTicker[];
}

const TICKER_PRESETS: Record<string, string[]> = {
  "Majors (8)": ["BTC-USDC", "ETH-USDC", "SOL-USDC", "AVAX-USDC", "BNB-USDC", "ARB-USDC", "OP-USDC", "SUI-USDC"],
  "Top 12": ["BTC-USDC", "ETH-USDC", "SOL-USDC", "AVAX-USDC", "BNB-USDC", "ARB-USDC", "OP-USDC", "SUI-USDC", "NEAR-USDC", "LINK-USDC", "DOGE-USDC", "PEPE-USDC"],
  "L1s": ["BTC-USDC", "ETH-USDC", "SOL-USDC", "AVAX-USDC", "BNB-USDC", "SUI-USDC", "NEAR-USDC", "APT-USDC", "SEI-USDC", "TON-USDC"],
  "DeFi": ["UNI-USDC", "AAVE-USDC", "CRV-USDC", "MKR-USDC", "LDO-USDC", "JUP-USDC", "PENDLE-USDC", "PYTH-USDC", "LINK-USDC"],
  "All (52)": PERP_TICKERS.map((t) => t.symbol),
};

export const LiveMatrix: React.FC<LiveMatrixProps> = ({
  symbol,
  orderbooks,
  opportunities,
  onRefresh,
  onExecuteTrade,
  onExecuteCrossTicker,
  loading,
  executionLogs = [],
  onOpenTickerConfig,
  activeVenueCodes,
  availableTickers,
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
  const [matrixMode, setMatrixMode] = useState<MatrixMode>("cross-venue");
  const [windowOption, setWindowOption] = useState<SlidingWindowOption>("1m");
  const [orderQty, setOrderQty] = useState(() => getDefaultOrderQty(activeTicker.basePrice));
  const [minBps, setMinBps] = useState(5.0);
  const [selectedPreset, setSelectedPreset] = useState<string>("Majors (8)");
  const [selectedPair, setSelectedPair] = useState<{ rowId: string; colId: string } | null>(null);
  const [executing, setExecuting] = useState(false);
  const [executionResult, setExecutionResult] = useState<{ success: boolean; message: string } | null>(null);
  const [autoExecute, setAutoExecute] = useState(true);
  const [lastAutoExecMsg, setLastAutoExecMsg] = useState<string | null>(null);
  const lastAutoExecTimeRef = React.useRef<number>(0);

  // Multi-ticker quotes state for Cross-Ticker matrix
  const [tickerQuotesMap, setTickerQuotesMap] = useState<Map<string, TickerMarketQuote>>(new Map());

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
      userTargetLeverage: 10,
    };
  }, [symbol, strategyConfigs, activeTicker.venues, activeTicker.basePrice, minBps]);

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

  // Derive active tickers for cross-ticker matrix
  const activeCrossTickers: PerpTicker[] = useMemo(() => {
    const symbolList = TICKER_PRESETS[selectedPreset] || TICKER_PRESETS["Majors (8)"];
    const pool = availableTickers ?? PERP_TICKERS;
    const poolSymbols = new Set(pool.map((t) => t.symbol));
    const set = new Set(symbolList.filter((s) => poolSymbols.has(s)));
    if (poolSymbols.has(symbol)) {
      set.add(symbol);
    }

    return pool.filter((t) => set.has(t.symbol));
  }, [selectedPreset, symbol, availableTickers]);

  // Populate and update quotes for active cross-tickers via real multi-market backend/mock quotes
  useEffect(() => {
    let isMounted = true;

    async function loadCrossTickerQuotes() {
      try {
        const symbolsToFetch = activeCrossTickers.map((t) => t.symbol);
        const quotes = await api.fetchMultiMarketQuotes(symbolsToFetch);

        if (!isMounted) return;

        const map = new Map<string, TickerMarketQuote>();
        for (const q of quotes) {
          map.set(q.symbol, {
            symbol: q.symbol,
            base: q.base,
            bestBid: q.best_bid,
            bestAsk: q.best_ask,
            midPrice: q.mid_price,
            spreadBps: q.spread_bps,
            bestBuyVenue: q.best_buy_venue,
            bestSellVenue: q.best_sell_venue,
            feeBps: q.fee_bps,
            basePrice: q.base_price,
          });
        }

        // Overlay active orderbooks for the selected symbol if already available
        if (orderbooks.length > 0) {
          let bestBid = -Infinity;
          let bestBidVenue = "Hyperliquid";
          let bestAsk = Infinity;
          let bestBuyVenue = "Hyperliquid";
          let bestBuySpread = 2.0;

          for (const b of orderbooks) {
            if (b.best_bid > bestBid) {
              bestBid = b.best_bid;
              bestBidVenue = b.venue_name;
            }
            if (b.best_ask < bestAsk && b.best_ask > 0) {
              bestAsk = b.best_ask;
              bestBuyVenue = b.venue_name;
              bestBuySpread = b.spread_bps > 0 ? b.spread_bps : ((b.best_ask - b.best_bid) / b.mid_price) * 10000;
            }
          }

          const mid = (bestBid + bestAsk) / 2;
          map.set(symbol, {
            symbol,
            base: symbol.split("-")[0],
            bestBid: bestBid > 0 ? bestBid : activeTicker.basePrice * 0.9998,
            bestAsk: bestAsk < Infinity ? bestAsk : activeTicker.basePrice * 1.0002,
            midPrice: mid > 0 ? mid : activeTicker.basePrice,
            spreadBps: Math.max(0.5, bestBuySpread),
            bestBuyVenue,
            bestSellVenue: bestBidVenue,
            feeBps: 2.0,
            basePrice: activeTicker.basePrice,
          });
        }

        setTickerQuotesMap(map);
      } catch (err) {
        console.error("Failed to load cross ticker quotes:", err);
      }
    }

    loadCrossTickerQuotes();

    return () => {
      isMounted = false;
    };
  }, [activeCrossTickers, orderbooks, symbol, activeTicker.basePrice]);

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

  // Build Cross-Ticker Matrix
  const crossTickerMatrix = useMemo(() => {
    const tradeNotional = orderQty * activeTicker.basePrice;
    return buildCrossTickerMatrix(
      activeCrossTickers,
      tickerQuotesMap,
      windowSeconds,
      tradeNotional,
      minBps,
      currentStrategyConfig,
      globalSpreadTracker
    );
  }, [activeCrossTickers, tickerQuotesMap, windowSeconds, orderQty, activeTicker.basePrice, minBps, currentStrategyConfig]);

  // Derive selected cell cleanly from selectedPair coordinates and current active matrix grid
  const selectedCell = useMemo<MatrixCellDto | null>(() => {
    if (!selectedPair) return null;
    const activeGrid = matrixMode === "cross-venue" ? venueMatrix.grid : crossTickerMatrix.grid;
    for (const row of activeGrid) {
      for (const cell of row) {
        if (cell.rowId === selectedPair.rowId && cell.colId === selectedPair.colId) {
          return cell;
        }
      }
    }
    return null;
  }, [selectedPair, matrixMode, venueMatrix.grid, crossTickerMatrix.grid]);

  // Execution handler
  const handleExecuteCellTrade = async (cell: MatrixCellDto) => {
    if (cell.isDiagonal || executing) return;
    setExecuting(true);
    setExecutionResult(null);

    try {
      if (matrixMode === "cross-venue") {
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
          direction: cell.isConvergenceUnwind ? "Convergence Unwind" : "Divergence Entry",
          trigger_spread_bps: cell.netSpreadBps,
          baseline_mean_spread_bps: cell.baselineMeanBps,
          position_step: cell.isConvergenceUnwind
            ? currentStrategyConfig.currentAccumulatedUnits
            : currentStrategyConfig.currentAccumulatedUnits + 1,
          max_units: currentStrategyConfig.maxAccumulatedUnits,
          nominal_hurdle_bps: cell.nominalHurdleBps,
        };
        await onExecuteTrade(opp);

        // Update accumulated units
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

        setExecutionResult({
          success: true,
          message: `${cell.isConvergenceUnwind ? "Unwound convergence" : "Filled atomically"}! Buy ${cell.longVenue} / Sell ${cell.shortVenue} (+${formatCurrencyPnl(cell.estProfitUsd)})`,
        });
      } else {
        const params: CrossTickerExecutionParams = {
          longSymbol: cell.longSymbol,
          shortSymbol: cell.shortSymbol,
          longVenue: cell.longVenue,
          shortVenue: cell.shortVenue,
          orderQty: cell.orderQty,
          estProfitUsd: cell.estProfitUsd,
          netSpreadBps: cell.netSpreadBps,
        };
        if (onExecuteCrossTicker) {
          await onExecuteCrossTicker(params);
        } else {
          await api.executeCrossTickerArbitrage(params);
          onRefresh(orderQty, minBps);
        }

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

        setExecutionResult({
          success: true,
          message: `${cell.isConvergenceUnwind ? "Unwound convergence pair" : "Executed pair trade"}! Long ${cell.rowLabel} / Short ${cell.colLabel} (+${formatCurrencyPnl(cell.estProfitUsd)})`,
        });
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setExecutionResult({
        success: false,
        message: `Execution rejected: ${msg}`,
      });
    } finally {
      setExecuting(false);
      setTimeout(() => setExecutionResult(null), 5000);
    }
  };

  // Automated Execution Engine Loop for Futures Spread Convergence
  useEffect(() => {
    if (!autoExecute || executing) return;

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
      setLastAutoExecMsg(
        `⚡ Auto-Executed ${bestCell.longSymbol}: Long ${bestCell.longVenue} / Short ${bestCell.shortVenue} (+${bestCell.netSpreadBps.toFixed(1)} BPS Net)`
      );
      handleExecuteCellTrade(bestCell);
    }
  }, [autoExecute, executing, venueMatrix.grid, currentStrategyConfig]);

  // Derive best off-diagonal opportunity from active matrix grid
  const bestMatrixCell = useMemo<MatrixCellDto | null>(() => {
    const activeGrid = matrixMode === "cross-venue" ? venueMatrix.grid : crossTickerMatrix.grid;
    let best: MatrixCellDto | null = null;
    for (const row of activeGrid) {
      for (const cell of row) {
        if (cell.isDiagonal) continue;
        if (!best || cell.netSpreadBps > best.netSpreadBps) {
          best = cell;
        }
      }
    }
    return best;
  }, [matrixMode, venueMatrix.grid, crossTickerMatrix.grid]);

  // Active cell displayed in the Instant Atomic Execution Dock (selected cell or top dislocation)
  const activeDockCell = selectedCell && !selectedCell.isDiagonal ? selectedCell : bestMatrixCell;

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

  const safeLeverage = useMemo(() => {
    return computeSafeLeverage(
      symbol,
      activeTicker.venues,
      activeTicker.basePrice,
      currentStrategyConfig.unitSize,
      currentStrategyConfig.userTargetLeverage
    );
  }, [symbol, activeTicker.venues, activeTicker.basePrice, currentStrategyConfig]);

  const [bottomTab, setBottomTab] = useState<"opportunities" | "history">("opportunities");

  return (
    <div className="p-3.5 space-y-3 overflow-y-auto h-full text-slate-900 font-sans">
      {/* 1. WeBull Ticker & Control Ribbon */}
      <div className="bg-surface rounded-xl border border-surface-border p-2.5 shadow-2xs flex flex-wrap items-center justify-between gap-2.5 text-xs font-mono select-none">
        {/* Left: Ticker identity, top price, max dislocation, and strategy chips */}
        <div className="flex flex-wrap items-center gap-2">
          {/* Symbol & Venue Pills */}
          <div className="flex items-center space-x-1.5 bg-surface-raised px-2.5 py-1 rounded-lg border border-surface-border">
            <span className="font-bold text-slate-900 text-sm">{symbol}</span>
            <span className="text-[10px] font-semibold px-1.5 py-0.2 bg-blue-50 text-accent rounded border border-blue-200">
              PERP
            </span>
            <div className="flex items-center space-x-1 ml-1">
              {activeTicker.venues.map((v) => (
                <span
                  key={v}
                  className="text-[9px] px-1 py-0.2 rounded font-semibold bg-white text-slate-600 border border-surface-border"
                >
                  {v}
                </span>
              ))}
            </div>
          </div>

          {/* Top-of-Book Mid Price & Spread */}
          {orderbooks.length > 0 && (
            <div className="flex items-center space-x-2 bg-surface-raised px-2.5 py-1 rounded-lg border border-surface-border">
              <span className="text-slate-400 text-[11px]">Mid:</span>
              <span className="font-bold text-slate-900">
                {formatCurrencyPrice(orderbooks[0]?.mid_price || activeTicker.basePrice)}
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
          <div className="hidden lg:flex items-center space-x-1.5 text-[11px] font-mono">
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
            <span
              className="px-1.5 py-0.5 rounded bg-surface-raised border border-surface-border text-slate-700 font-medium"
              title={`Nominal Hurdle: ${nominalHurdleBps.toFixed(1)} BPS`}
            >
              Hurdle: {nominalHurdleBps.toFixed(1)} bps
            </span>
          </div>

          {/* Quick Config Button */}
          {onOpenTickerConfig && (
            <button
              onClick={() => onOpenTickerConfig(symbol, "futures")}
              className="flex items-center space-x-1 px-2 py-1 bg-surface hover:bg-surface-raised text-slate-700 font-medium rounded-md border border-surface-border text-xs transition"
              title="Configure strategy parameters"
            >
              <SlidersHorizontal className="w-3 h-3 text-slate-500" />
              <span className="text-[11px]">Config</span>
            </button>
          )}
        </div>

        {/* Right: Controls (Matrix Mode, Sliding Window, Auto-Bot, Scan) */}
        <div className="flex flex-wrap items-center gap-2">
          {/* Mode Toggle */}
          <div className="flex items-center bg-surface-raised p-0.5 rounded-lg border border-surface-border">
            <button
              onClick={() => {
                setMatrixMode("cross-venue");
                setSelectedPair(null);
              }}
              className={`flex items-center space-x-1 px-2.5 py-1 rounded text-xs font-medium transition ${
                matrixMode === "cross-venue"
                  ? "bg-surface text-accent shadow-2xs font-semibold border border-surface-border"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <Grid3X3 className="w-3 h-3" />
              <span>Cross-Venue</span>
            </button>
            <button
              onClick={() => {
                setMatrixMode("cross-ticker");
                setSelectedPair(null);
              }}
              className={`flex items-center space-x-1 px-2.5 py-1 rounded text-xs font-medium transition ${
                matrixMode === "cross-ticker"
                  ? "bg-surface text-accent shadow-2xs font-semibold border border-surface-border"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <Layers className="w-3 h-3" />
              <span>Cross-Ticker</span>
            </button>
          </div>

          {/* Preset (if cross-ticker) */}
          {matrixMode === "cross-ticker" && (
            <select
              value={selectedPreset}
              onChange={(e) => {
                setSelectedPreset(e.target.value);
                setSelectedPair(null);
              }}
              className="bg-surface border border-surface-border rounded-md px-2 py-1 text-xs font-mono text-slate-800 focus:outline-none"
            >
              {Object.keys(TICKER_PRESETS).map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          )}

          {/* Sliding Window */}
          <div className="flex items-center space-x-0.5 bg-surface-raised p-0.5 rounded-lg border border-surface-border text-xs">
            {(["10s", "30s", "1m", "5m", "15m"] as SlidingWindowOption[]).map((w) => (
              <button
                key={w}
                onClick={() => setWindowOption(w)}
                className={`px-1.5 py-0.5 rounded text-[11px] font-mono transition ${
                  windowOption === w
                    ? "bg-accent text-white font-bold shadow-2xs"
                    : "text-slate-600 hover:text-slate-900"
                }`}
                title={`Rolling ${w} window`}
              >
                {w}
              </button>
            ))}
          </div>

          {/* Auto-Bot Switch Toggle (Green for ON, White for OFF) */}
          <div className="flex items-center space-x-1.5 px-2 py-0.5 bg-surface border border-surface-border rounded-lg shadow-2xs">
            <span className="text-xs font-mono font-medium text-slate-700">Auto</span>
            <SwitchToggle
              checked={autoExecute}
              onChange={setAutoExecute}
              size="sm"
              showStatusText={true}
            />
          </div>

          {/* Scan Button */}
          <button
            onClick={() => onRefresh(orderQty, minBps)}
            disabled={loading}
            className="flex items-center space-x-1 px-2.5 py-1 bg-surface hover:bg-surface-raised border border-surface-border rounded-lg text-xs font-medium text-slate-700 transition shadow-2xs"
            title="Scan orderbooks now"
          >
            <RefreshCw
              className={`w-3 h-3 ${loading ? "animate-spin text-accent" : "text-slate-500"}`}
            />
            <span>Scan</span>
          </button>
        </div>
      </div>

      {/* Auto-execution notification notice */}
      {lastAutoExecMsg && (
        <div className="px-3 py-1.5 bg-emerald-50 border border-emerald-200 rounded-lg text-xs font-mono text-emerald-800 flex items-center justify-between shadow-2xs">
          <span>{lastAutoExecMsg}</span>
          <button
            onClick={() => setLastAutoExecMsg(null)}
            className="text-emerald-600 hover:text-emerald-900 text-xs font-bold ml-2"
          >
            ×
          </button>
        </div>
      )}

      {/* 2. Strategy Parameters Ribbon */}
      <TickerStrategyBar
        ticker={activeTicker}
        venues={activeTicker.venues}
        config={currentStrategyConfig}
        onChangeConfig={handleUpdateStrategyConfig}
        onOpenConfigModal={onOpenTickerConfig ? () => onOpenTickerConfig(symbol, "futures") : undefined}
      />

      {/* 3. Main Stage: Dual-Split Layout (Left: 2D Matrix Grid, Right: Consolidated Quotes + Instant Dock) */}
      <div className="grid grid-cols-12 gap-3 items-start">
        {/* Left: 2D Interactive Arbitrage Matrix */}
        <div className="col-span-12 xl:col-span-7 bg-surface rounded-xl border border-surface-border p-3.5 shadow-2xs space-y-3">
          {/* Header with Title, Quick Min BPS, and Quant Legend */}
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-surface-border pb-2.5">
            <div>
              <h3 className="text-xs font-bold font-mono text-slate-900 flex items-center space-x-2">
                <span>
                  {matrixMode === "cross-venue"
                    ? `Arbitrage Dislocation Matrix — [${symbol}]`
                    : `Cross-Ticker Relative Value — [${selectedPreset}]`}
                </span>
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
            {matrixMode === "cross-venue" ? (
              venueMatrix.grid.length < 2 ? (
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
              )
            ) : (
              /* Cross-Ticker Matrix Grid */
              crossTickerMatrix.grid.length === 0 ? (
                <div className="py-12 text-center text-slate-400 font-mono text-xs">
                  Building multi-market matrix for {selectedPreset}...
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
                      {crossTickerMatrix.symbolLabels.map((colSymbol, j) => {
                        const sym = crossTickerMatrix.symbols[j];
                        const quote = tickerQuotesMap.get(sym);
                        return (
                          <th
                            key={sym}
                            className="p-2 text-center font-bold text-slate-800 border-r border-b border-surface-border last:border-r-0 min-w-[110px] sticky top-0 bg-surface-raised z-20"
                          >
                            <div className="text-[11px] text-slate-900">{colSymbol}</div>
                            <div className="text-[10px] font-normal text-profit mt-0.5">
                              Bid: {quote ? formatCurrencyPrice(quote.bestBid) : "..."}
                            </div>
                          </th>
                        );
                      })}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-surface-border">
                    {crossTickerMatrix.grid.map((row, i) => {
                      const rowLabel = crossTickerMatrix.symbolLabels[i];
                      const rowSym = crossTickerMatrix.symbols[i];
                      const quote = tickerQuotesMap.get(rowSym);

                      return (
                        <tr key={rowSym} className="hover:bg-surface-raised/40 transition">
                          <td className="p-2.5 font-semibold text-slate-900 border-r border-surface-border bg-surface-raised/95 sticky left-0 z-10">
                            <div className="text-[11px] font-bold text-slate-900">{rowLabel}</div>
                            <div className="text-[10px] font-normal text-loss mt-0.5">
                              Ask: {quote ? formatCurrencyPrice(quote.bestAsk) : "..."}
                            </div>
                          </td>

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
              )
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

          {/* Widget 2: Instant Atomic Execution Dock */}
          <div className="bg-surface rounded-xl border border-surface-border p-3.5 shadow-2xs space-y-3">
            <div className="flex items-center justify-between border-b border-surface-border pb-2">
              <div className="flex items-center space-x-2">
                <Zap className="w-3.5 h-3.5 text-accent" />
                <span className="font-mono text-xs font-bold text-slate-900">
                  Instant Atomic Execution Dock
                </span>
                <span
                  className={`px-1.5 py-0.2 rounded text-[10px] font-mono font-semibold ${
                    selectedCell
                      ? "bg-blue-50 text-accent border border-blue-200"
                      : "bg-surface-raised text-slate-600 border border-surface-border"
                  }`}
                >
                  {selectedCell ? "Selected Cell" : "Top Dislocation (Auto)"}
                </span>
              </div>
              {selectedCell && (
                <button
                  onClick={() => setSelectedPair(null)}
                  className="text-[10px] font-mono text-slate-500 hover:text-accent underline transition"
                  title="Reset to top opportunity"
                >
                  Reset to Top
                </button>
              )}
            </div>

            {activeDockCell ? (
              <div className="space-y-3">
                {/* Route Header */}
                <div className="bg-surface-raised p-2.5 rounded-lg border border-surface-border font-mono text-xs flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center space-x-2">
                    <span className="px-1.5 py-0.5 rounded bg-loss-dim text-loss font-bold border border-loss-border text-[10px]">
                      LONG
                    </span>
                    <span className="font-bold text-slate-900">{activeDockCell.rowLabel}</span>
                    <span className="text-slate-500">({activeDockCell.longVenue})</span>
                    <span className="text-loss font-semibold">
                      @{formatCurrencyPrice(activeDockCell.longPrice)}
                    </span>
                  </div>

                  <ArrowRight className="w-3.5 h-3.5 text-slate-400" />

                  <div className="flex items-center space-x-2">
                    <span className="px-1.5 py-0.5 rounded bg-profit-dim text-profit font-bold border border-profit-border text-[10px]">
                      SHORT
                    </span>
                    <span className="font-bold text-slate-900">{activeDockCell.colLabel}</span>
                    <span className="text-slate-500">({activeDockCell.shortVenue})</span>
                    <span className="text-profit font-semibold">
                      @{formatCurrencyPrice(activeDockCell.shortPrice)}
                    </span>
                  </div>
                </div>

                {/* 4 Quant Metrics Grid */}
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-2 font-mono text-center">
                  <div className="bg-surface-raised p-2 rounded-lg border border-surface-border">
                    <div className="text-[10px] text-slate-400 uppercase">Gross Spread</div>
                    <div className="text-xs font-bold text-slate-900 mt-0.5">
                      +{activeDockCell.grossSpreadBps.toFixed(1)} BPS
                    </div>
                  </div>

                  <div className="bg-surface-raised p-2 rounded-lg border border-surface-border">
                    <div className="text-[10px] text-slate-400 uppercase">Friction</div>
                    <div className="text-xs font-bold text-loss mt-0.5">
                      -{(activeDockCell.feeBps + activeDockCell.closingFrictionBps).toFixed(1)} BPS
                    </div>
                  </div>

                  <div className="bg-surface-raised p-2 rounded-lg border border-surface-border">
                    <div className="text-[10px] text-slate-400 uppercase">Net Spread</div>
                    <div
                      className={`text-xs font-bold mt-0.5 ${
                        activeDockCell.netSpreadBps >= minBps
                          ? "text-profit"
                          : activeDockCell.netSpreadBps > 0
                          ? "text-accent"
                          : "text-slate-500"
                      }`}
                    >
                      {activeDockCell.netSpreadBps > 0 ? "+" : ""}
                      {activeDockCell.netSpreadBps.toFixed(1)} BPS
                    </div>
                  </div>

                  <div className="bg-surface-raised p-2 rounded-lg border border-surface-border">
                    <div className="text-[10px] text-slate-400 uppercase">Est. Profit</div>
                    <div
                      className={`text-xs font-bold mt-0.5 ${
                        activeDockCell.estProfitUsd >= 0 ? "text-profit" : "text-loss"
                      }`}
                    >
                      {formatCurrencyPnl(activeDockCell.estProfitUsd)}
                    </div>
                  </div>
                </div>

                {/* Order Sizing & Margin Row */}
                <div className="flex items-center justify-between text-xs font-mono bg-surface-raised p-2 rounded-lg border border-surface-border">
                  <div className="flex items-center space-x-2">
                    <span className="text-slate-500">Qty:</span>
                    <input
                      type="number"
                      step={orderQty >= 1000 ? "100" : orderQty >= 10 ? "1" : orderQty >= 1 ? "0.1" : "0.001"}
                      min="0.00000001"
                      value={orderQty}
                      onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
                      className="w-20 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent"
                    />
                    <span className="text-slate-500">{activeTicker.base}</span>
                  </div>

                  <div className="text-[11px] text-slate-600">
                    Lev: <strong className="text-slate-800">{safeLeverage.effectiveLeverage}x</strong> · Req:{" "}
                    <strong className="text-slate-800">
                      ${((safeLeverage.totalMarginRequiredUsd * orderQty) / (currentStrategyConfig.unitSize || 1)).toFixed(0)}
                    </strong>
                  </div>
                </div>

                {/* Execution Result Feedback */}
                {executionResult && (
                  <div
                    className={`text-xs font-mono px-2.5 py-1.5 rounded-lg border flex items-center space-x-1.5 ${
                      executionResult.success
                        ? "bg-profit-dim text-profit border-profit-border"
                        : "bg-loss-dim text-loss border-loss-border"
                    }`}
                  >
                    {executionResult.success && <CheckCircle2 className="w-3.5 h-3.5 shrink-0" />}
                    <span className="truncate">{executionResult.message}</span>
                  </div>
                )}

                {/* One-Click Execute Button */}
                <button
                  onClick={() => handleExecuteCellTrade(activeDockCell)}
                  disabled={executing || loading}
                  className={`w-full py-2.5 px-4 rounded-lg text-xs font-mono font-bold transition flex items-center justify-center space-x-2 shadow-2xs ${
                    executing
                      ? "bg-slate-200 text-slate-500 cursor-not-allowed"
                      : activeDockCell.isExecutable
                      ? "bg-accent hover:bg-accent-hover text-white cursor-pointer"
                      : "bg-surface-raised hover:bg-surface border border-surface-border text-slate-800 cursor-pointer"
                  }`}
                >
                  <Zap className={`w-3.5 h-3.5 ${executing ? "animate-spin" : ""}`} />
                  <span>
                    {executing
                      ? "Routing Concurrent IOC Orders..."
                      : `Execute Atomic Arbitrage (${activeDockCell.rowLabel} ➔ ${activeDockCell.colLabel})`}
                  </span>
                </button>
              </div>
            ) : (
              <div className="py-8 text-center text-slate-400 font-mono text-xs">
                Scanning orderbooks for active arbitrage opportunities...
              </div>
            )}
          </div>
        </div>
      </div>

      {/* 4. Bottom Docked Tabbed Panel: Ranked Opportunities vs Execution History */}
      <div className="bg-surface rounded-xl border border-surface-border shadow-2xs overflow-hidden">
        {/* Tab Switcher */}
        <div className="flex items-center space-x-1 border-b border-surface-border px-4 pt-2 bg-surface-raised">
          <button
            onClick={() => setBottomTab("opportunities")}
            className={`flex items-center space-x-1.5 px-3 py-2 text-xs font-mono font-semibold border-b-2 transition ${
              bottomTab === "opportunities"
                ? "border-accent text-accent bg-surface"
                : "border-transparent text-slate-500 hover:text-slate-900"
            }`}
          >
            <Zap className="w-3.5 h-3.5" />
            <span>Ranked Opportunities</span>
            <span className="px-1.5 py-0.2 text-[10px] rounded-full bg-blue-50 text-accent border border-blue-200">
              {opportunities.length}
            </span>
          </button>
          <button
            onClick={() => setBottomTab("history")}
            className={`flex items-center space-x-1.5 px-3 py-2 text-xs font-mono font-semibold border-b-2 transition ${
              bottomTab === "history"
                ? "border-accent text-accent bg-surface"
                : "border-transparent text-slate-500 hover:text-slate-900"
            }`}
          >
            <History className="w-3.5 h-3.5" />
            <span>Execution History & Telemetry</span>
            <span className="px-1.5 py-0.2 text-[10px] rounded-full bg-surface-raised text-slate-600 border border-surface-border">
              {executionLogs?.length || 0}
            </span>
          </button>
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
          ) : (
            <div className="max-h-96 overflow-y-auto">
              <TickerTransactionHistory
                symbol={symbol}
                logs={executionLogs || []}
                config={currentStrategyConfig}
                onRefresh={() => onRefresh(orderQty, minBps)}
              />
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
