import React, { useState, useEffect, useMemo } from "react";
import {
  RefreshCw,
  TrendingUp,
  TrendingDown,
  Layers,
  Grid3X3,
  Zap,
  CheckCircle2,
  X,
  ChevronDown,
  ChevronUp,
  Info,
  ArrowRight,
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
  formatSpreadPrice,
  getDefaultOrderQty,
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
import { getMinCompatibleSize } from "../services/contractSpecs";
import { TickerStrategyBar } from "./TickerStrategyBar";
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
}) => {
  const activeTicker = PERP_TICKERS.find((t) => t.symbol === symbol) || {
    symbol,
    base: symbol.split("-")[0],
    quote: symbol.split("-")[1] || "USDC",
    name: symbol,
    venues: ["HL", "LGT", "VNT"],
    basePrice: 100,
  };

  // State
  const [matrixMode, setMatrixMode] = useState<MatrixMode>("cross-venue");
  const [windowOption, setWindowOption] = useState<SlidingWindowOption>("1m");
  const [orderQty, setOrderQty] = useState(() => getDefaultOrderQty(activeTicker.basePrice));
  const [minBps, setMinBps] = useState(5.0);
  const [selectedPreset, setSelectedPreset] = useState<string>("Majors (8)");
  const [selectedPair, setSelectedPair] = useState<{ rowId: string; colId: string } | null>(null);
  const [executing, setExecuting] = useState(false);
  const [executionResult, setExecutionResult] = useState<{ success: boolean; message: string } | null>(null);
  const [showRankedList, setShowRankedList] = useState(false);

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
    // Ensure currently selected symbol is always included
    const set = new Set(symbolList);
    set.add(symbol);

    return PERP_TICKERS.filter((t) => set.has(t.symbol));
  }, [selectedPreset, symbol]);

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

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full text-slate-900">
      {/* Top Bar: Mode Selector & Quant Controls */}
      <div className="flex flex-wrap items-center justify-between gap-4 bg-surface p-4 rounded-xl border border-surface-border shadow-xs">
        {/* Left Side: Mode Toggle & Active Ticker */}
        <div className="flex flex-wrap items-center gap-3">
          {/* Matrix Mode Toggle */}
          <div className="flex items-center bg-surface-raised p-1 rounded-lg border border-surface-border">
            <button
              onClick={() => {
                setMatrixMode("cross-venue");
                setSelectedPair(null);
              }}
              className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-mono font-medium transition ${
                matrixMode === "cross-venue"
                  ? "bg-surface text-accent shadow-xs border border-surface-border"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <Grid3X3 className="w-3.5 h-3.5" />
              <span>Cross-Venue Matrix</span>
            </button>
            <button
              onClick={() => {
                setMatrixMode("cross-ticker");
                setSelectedPair(null);
              }}
              className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-mono font-medium transition ${
                matrixMode === "cross-ticker"
                  ? "bg-surface text-accent shadow-xs border border-surface-border"
                  : "text-slate-600 hover:text-slate-900"
              }`}
            >
              <Layers className="w-3.5 h-3.5" />
              <span>Cross-Ticker Matrix</span>
            </button>
          </div>

          {/* Active Ticker Badge */}
          <div className="text-xs font-mono font-bold text-slate-900 bg-surface-raised px-2.5 py-1.5 rounded-md border border-surface-border flex items-center space-x-2">
            <span>{symbol}</span>
            <span className="flex space-x-1">
              {activeTicker.venues.map((v) => (
                <span
                  key={v}
                  className="text-[9px] px-1 py-0.2 rounded font-semibold bg-accent-dim text-accent border border-accent-line/40"
                >
                  {v}
                </span>
              ))}
            </span>
            <span
              className="text-[10px] px-1.5 py-0.2 rounded bg-surface border border-surface-border font-medium text-slate-700"
              title={`${tickerStats.count} strategy executions`}
            >
              {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
            </span>
            <span
              className={`text-[10px] px-1.5 py-0.2 rounded font-semibold ${
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

          {/* Cross-Ticker Presets (only shown in Cross-Ticker mode) */}
          {matrixMode === "cross-ticker" && (
            <div className="flex items-center space-x-1.5 bg-surface-raised px-2 py-1 rounded-md border border-surface-border text-xs font-mono">
              <span className="text-slate-500 text-[11px]">Preset:</span>
              <select
                value={selectedPreset}
                onChange={(e) => {
                  setSelectedPreset(e.target.value);
                  setSelectedPair(null);
                }}
                className="bg-surface border border-surface-border rounded px-2 py-0.5 text-xs font-mono text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent"
              >
                {Object.keys(TICKER_PRESETS).map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
            </div>
          )}
        </div>

        {/* Right Side: Sliding Window Selector, Inputs, and Scan */}
        <div className="flex flex-wrap items-center gap-3">
          {/* Predefined Sliding Window Selector */}
          <div className="flex items-center space-x-1 bg-surface-raised p-1 rounded-lg border border-surface-border text-xs font-mono">
            <span className="text-slate-500 text-[10px] uppercase font-semibold px-1.5">Window:</span>
            {(["10s", "30s", "1m", "5m", "15m"] as SlidingWindowOption[]).map((w) => (
              <button
                key={w}
                onClick={() => setWindowOption(w)}
                className={`px-2 py-1 rounded text-xs font-mono font-medium transition ${
                  windowOption === w
                    ? "bg-accent text-white shadow-xs"
                    : "text-slate-600 hover:text-slate-900 hover:bg-surface"
                }`}
                title={`Track baseline over rolling ${w}`}
              >
                {w}
              </button>
            ))}
          </div>

          {/* Benchmark Order Qty */}
          <div className="flex items-center space-x-1.5 text-xs font-mono bg-surface-raised px-2 py-1 rounded-md border border-surface-border">
            <span className="text-slate-500">Qty:</span>
            <input
              type="number"
              step={orderQty >= 1000 ? "100" : orderQty >= 10 ? "1" : orderQty >= 1 ? "0.1" : "0.001"}
              min="0.00000001"
              value={orderQty}
              onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
              className="w-20 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </div>

          {/* Min Net BPS Filter */}
          <div className="flex items-center space-x-1.5 text-xs font-mono bg-surface-raised px-2 py-1 rounded-md border border-surface-border">
            <span className="text-slate-500">Min BPS:</span>
            <input
              type="number"
              step="1"
              value={minBps}
              onChange={(e) => setMinBps(parseFloat(e.target.value) || 0)}
              className="w-14 px-1.5 py-0.5 bg-surface border border-surface-border rounded text-slate-900 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-accent"
            />
          </div>

          {/* Refresh Button */}
          <button
            onClick={() => onRefresh(orderQty, minBps)}
            disabled={loading}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-surface hover:bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-700 font-medium transition shadow-xs"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-accent" : "text-slate-500"}`} />
            <span>Scan</span>
          </button>
        </div>
      </div>

      {/* Quantitative Ticker Strategy & Sizing Control Bar */}
      <TickerStrategyBar
        ticker={activeTicker}
        venues={activeTicker.venues}
        config={currentStrategyConfig}
        onChangeConfig={handleUpdateStrategyConfig}
      />

      {/* 4-Venue Market Quotes Grid (Shown in Cross-Venue mode) */}
      {matrixMode === "cross-venue" && (
        <div>
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-xs font-mono uppercase tracking-wider text-slate-500 font-semibold">
              Venue Market Quotes (Top of Book)
            </h3>
            <span className="text-[11px] font-mono text-slate-400">Target: {symbol}</span>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
            {orderbooks.map((book) => {
              const spreadColor = book.spread_bps < 10 ? "text-profit" : "text-warn";
              const topBidSize = book.bids.length > 0 ? book.bids[0][1] : 0;
              const topAskSize = book.asks.length > 0 ? book.asks[0][1] : 0;

              return (
                <div
                  key={book.venue_id}
                  className="bg-surface border border-surface-border rounded-xl p-4 flex flex-col justify-between hover:border-slate-300 transition shadow-xs"
                >
                  <div className="flex items-center justify-between border-b border-surface-border pb-2.5 mb-3">
                    <div>
                      <span className="font-mono text-xs font-bold text-slate-900">{book.venue_name}</span>
                      <div className="text-[10px] font-mono text-slate-500 mt-0.5">
                        8h Funding: {book.funding_rate_8h_pct.toFixed(4)}%
                      </div>
                    </div>
                    <div className="text-right">
                      <span className="text-[10px] font-mono text-slate-400">Spread:</span>{" "}
                      <span className={`font-mono text-xs font-semibold ${spreadColor}`}>
                        {book.spread_bps.toFixed(1)} BPS
                      </span>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 gap-2 bg-surface-raised p-2.5 rounded-lg border border-surface-border mb-3">
                    <div>
                      <div className="text-[10px] font-mono text-slate-400 uppercase tracking-wide flex justify-between">
                        <span>Bid (Buy)</span>
                        <span className="text-slate-500">{topBidSize > 0 ? topBidSize.toFixed(2) : ""}</span>
                      </div>
                      <div className="text-sm font-mono font-bold text-profit mt-1">
                        {formatCurrencyPrice(book.best_bid)}
                      </div>
                    </div>

                    <div className="border-l border-surface-border pl-2.5">
                      <div className="text-[10px] font-mono text-slate-400 uppercase tracking-wide flex justify-between">
                        <span>Ask (Sell)</span>
                        <span className="text-slate-500">{topAskSize > 0 ? topAskSize.toFixed(2) : ""}</span>
                      </div>
                      <div className="text-sm font-mono font-bold text-loss mt-1">
                        {formatCurrencyPrice(book.best_ask)}
                      </div>
                    </div>
                  </div>

                  <div className="pt-2 border-t border-surface-border text-[11px] font-mono text-slate-500 flex justify-between items-center">
                    <span>Mid Price</span>
                    <span className="font-semibold text-slate-700">{formatCurrencyPrice(book.mid_price)}</span>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Real 2D Interactive Matrix Grid */}
      <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-xs space-y-4">
        {/* Header with Title and Quant Legend */}
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-surface-border pb-3">
          <div>
            <h3 className="text-sm font-bold font-mono text-slate-900 flex items-center space-x-2">
              <span>
                {matrixMode === "cross-venue"
                  ? `Arbitrage Dislocation Matrix — [Cross-Venue: ${symbol}]`
                  : `Cross-Ticker Relative Value Matrix — [Preset: ${selectedPreset}]`}
              </span>
              <span className="px-2 py-0.5 rounded-full text-[10px] bg-accent-dim text-accent border border-accent-line font-medium">
                Rolling {windowOption} Window
              </span>
            </h3>
            <p className="text-xs text-slate-500 mt-1">
              Y-Axis = <strong>LONG (Buy Leg)</strong> · X-Axis = <strong>SHORT (Sell Leg)</strong>. Cell shows{" "}
              <strong>Net BPS</strong> after taker fees &amp; closing spread friction, and <strong>Δ BPS</strong> vs rolling {windowOption} baseline.
            </p>
          </div>

          {/* Color Legend */}
          <div className="flex items-center space-x-3 text-[11px] font-mono">
            <div className="flex items-center space-x-1">
              <span className="w-2.5 h-2.5 rounded bg-emerald-100 border border-emerald-400 inline-block"></span>
              <span className="text-emerald-800 font-medium">Net ≥ {minBps} BPS (Executable)</span>
            </div>
            <div className="flex items-center space-x-1">
              <span className="w-2.5 h-2.5 rounded bg-sky-100 border border-sky-300 inline-block"></span>
              <span className="text-sky-800 font-medium">0 &lt; Net &lt; {minBps} BPS</span>
            </div>
            <div className="flex items-center space-x-1">
              <span className="w-2.5 h-2.5 rounded bg-slate-100 border border-slate-300 inline-block"></span>
              <span className="text-slate-600">Net ≤ 0</span>
            </div>
            <div className="flex items-center space-x-1">
              <span className="w-2.5 h-2.5 rounded bg-surface-raised border border-surface-border inline-block"></span>
              <span className="text-slate-400">— (Diagonal)</span>
            </div>
          </div>
        </div>

        {/* The 2D Interactive Matrix Table */}
        <div className="overflow-auto max-h-[640px] rounded-lg border border-surface-border">
          {matrixMode === "cross-venue" ? (
            /* Cross-Venue Matrix Grid */
            venueMatrix.grid.length === 0 ? (
              <div className="py-12 text-center text-slate-400 font-mono text-xs">
                Scanning orderbooks for {symbol}...
              </div>
            ) : (
              <table className="w-full border-collapse font-mono text-xs">
                <thead className="sticky top-0 z-20 bg-surface-raised shadow-xs">
                  <tr className="bg-surface-raised border-b border-surface-border">
                    <th className="p-3 text-left font-semibold text-slate-500 uppercase text-[10px] tracking-wider border-r border-b border-surface-border min-w-[140px] sticky top-0 left-0 bg-surface-raised z-30">
                      <div className="flex items-center justify-between">
                        <span>LONG (Y) ↓</span>
                        <span>SHORT (X) →</span>
                      </div>
                    </th>
                    {venueMatrix.venueLabels.map((colVenue, j) => {
                      const sellPrice = orderbooks[j]?.best_bid ?? 0;
                      return (
                        <th
                          key={colVenue}
                          className="p-3 text-center font-bold text-slate-800 border-r border-b border-surface-border last:border-r-0 min-w-[130px] sticky top-0 bg-surface-raised z-20"
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
                        <td className="p-3 font-semibold text-slate-900 border-r border-surface-border bg-surface-raised/95 sticky left-0 z-10">
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
                                className="p-3 text-center border-r border-surface-border last:border-r-0 bg-surface-raised/40 select-none"
                              >
                                <span className="text-slate-400 font-mono text-base">—</span>
                              </td>
                            );
                          }

                          return (
                            <td
                              key={`${cell.rowId}-${cell.colId}`}
                              onClick={() => setSelectedPair({ rowId: cell.rowId, colId: cell.colId })}
                              className={`p-2.5 text-center border-r border-surface-border last:border-r-0 cursor-pointer transition select-none ${
                                visuals.bgClass
                              } ${isSelected ? "ring-2 ring-accent shadow-sm" : ""}`}
                              title={`Click to inspect & execute: Long ${cell.rowLabel} / Short ${cell.colLabel}`}
                            >
                              <div className="flex flex-col items-center justify-center space-y-1">
                                {/* Net Spread BPS */}
                                <div className={`text-xs font-mono font-bold ${visuals.textClass}`}>
                                  {cell.netSpreadBps > 0 ? "+" : ""}
                                  {cell.netSpreadBps.toFixed(1)} BPS
                                </div>

                                {/* Window Difference Δ BPS */}
                                <div
                                  className={`flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] font-mono ${visuals.deltaBadgeBg}`}
                                >
                                  {cell.deltaBps > 0 ? (
                                    <TrendingUp className="w-2.5 h-2.5 inline" />
                                  ) : cell.deltaBps < 0 ? (
                                    <TrendingDown className="w-2.5 h-2.5 inline" />
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
                <thead className="sticky top-0 z-20 bg-surface-raised shadow-xs">
                  <tr className="bg-surface-raised border-b border-surface-border">
                    <th className="p-3 text-left font-semibold text-slate-500 uppercase text-[10px] tracking-wider border-r border-b border-surface-border min-w-[140px] sticky top-0 left-0 bg-surface-raised z-30">
                      <div className="flex items-center justify-between">
                        <span>LONG (Y) ↓</span>
                        <span>SHORT (X) →</span>
                      </div>
                    </th>
                    {crossTickerMatrix.symbolLabels.map((colSymbol, j) => {
                      const sym = crossTickerMatrix.symbols[j];
                      const quote = tickerQuotesMap.get(sym);
                      return (
                        <th
                          key={sym}
                          className="p-3 text-center font-bold text-slate-800 border-r border-b border-surface-border last:border-r-0 min-w-[124px] sticky top-0 bg-surface-raised z-20"
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
                        {/* Row Header (Long Leg) */}
                        <td className="p-3 font-semibold text-slate-900 border-r border-surface-border bg-surface-raised/95 sticky left-0 z-10">
                          <div className="text-[11px] font-bold text-slate-900">{rowLabel}</div>
                          <div className="text-[10px] font-normal text-loss mt-0.5">
                            Ask: {quote ? formatCurrencyPrice(quote.bestAsk) : "..."}
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
                                className="p-3 text-center border-r border-surface-border last:border-r-0 bg-surface-raised/40 select-none"
                              >
                                <span className="text-slate-400 font-mono text-base">—</span>
                              </td>
                            );
                          }

                          return (
                            <td
                              key={`${cell.rowId}-${cell.colId}`}
                              onClick={() => setSelectedPair({ rowId: cell.rowId, colId: cell.colId })}
                              className={`p-2.5 text-center border-r border-surface-border last:border-r-0 cursor-pointer transition select-none ${
                                visuals.bgClass
                              } ${isSelected ? "ring-2 ring-accent shadow-sm" : ""}`}
                              title={`Click to inspect & execute: Long ${cell.rowLabel} / Short ${cell.colLabel}`}
                            >
                              <div className="flex flex-col items-center justify-center space-y-1">
                                <div className={`text-xs font-mono font-bold ${visuals.textClass}`}>
                                  {cell.netSpreadBps > 0 ? "+" : ""}
                                  {cell.netSpreadBps.toFixed(1)} BPS
                                </div>

                                <div
                                  className={`flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] font-mono ${visuals.deltaBadgeBg}`}
                                >
                                  {cell.deltaBps > 0 ? (
                                    <TrendingUp className="w-2.5 h-2.5 inline" />
                                  ) : cell.deltaBps < 0 ? (
                                    <TrendingDown className="w-2.5 h-2.5 inline" />
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

      {/* Selected Cell Trade Details & One-Click Execution Card */}
      {selectedCell && !selectedCell.isDiagonal && (
        <div className="bg-surface rounded-xl border border-accent-line/80 p-5 shadow-md bg-gradient-to-r from-surface to-accent-dim/20 space-y-4">
          <div className="flex items-center justify-between border-b border-surface-border pb-3">
            <div className="flex items-center space-x-2">
              <Zap className="w-4 h-4 text-accent" />
              <h4 className="text-sm font-bold font-mono text-slate-900">
                Atomic Execution &amp; Spread Analytics
              </h4>
              <span className="text-xs px-2 py-0.5 rounded font-mono font-semibold bg-surface border border-surface-border text-slate-700">
                {matrixMode === "cross-venue" ? "Cross-Venue Spatial Arbitrage" : "Cross-Ticker Statistical Relative Value"}
              </span>
            </div>
            <button
              onClick={() => setSelectedPair(null)}
              className="text-slate-400 hover:text-slate-700 p-1 rounded-md transition"
              title="Close panel"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          {/* Trade Route Banner */}
          <div className="flex flex-wrap items-center justify-between gap-4 bg-surface-raised p-3.5 rounded-lg border border-surface-border font-mono text-xs">
            <div className="flex items-center space-x-3">
              <div className="flex items-center space-x-1.5">
                <span className="px-2 py-0.5 rounded bg-loss-dim text-loss font-bold border border-loss-border">
                  BUY / LONG
                </span>
                <span className="font-bold text-slate-900">{selectedCell.rowLabel}</span>
                <span className="text-slate-500">({selectedCell.longVenue})</span>
                <span className="font-semibold text-loss">@{formatCurrencyPrice(selectedCell.longPrice)}</span>
              </div>

              <ArrowRight className="w-4 h-4 text-slate-400" />

              <div className="flex items-center space-x-1.5">
                <span className="px-2 py-0.5 rounded bg-profit-dim text-profit font-bold border border-profit-border">
                  SELL / SHORT
                </span>
                <span className="font-bold text-slate-900">{selectedCell.colLabel}</span>
                <span className="text-slate-500">({selectedCell.shortVenue})</span>
                <span className="font-semibold text-profit">@{formatCurrencyPrice(selectedCell.shortPrice)}</span>
              </div>
            </div>

            <div className="text-right">
              <span className="text-slate-500">Order Quantity: </span>
              <span className="font-bold text-slate-900">
                {selectedCell.orderQty} {selectedCell.longSymbol.split("-")[0]}
              </span>
            </div>
          </div>

          {/* Quant Breakdown Metric Cards */}
          <div className="grid grid-cols-2 md:grid-cols-5 gap-3 font-mono">
            {/* Gross Spread */}
            <div className="bg-surface p-3 rounded-lg border border-surface-border">
              <div className="text-[10px] text-slate-500 uppercase tracking-wide">Gross Spread</div>
              <div className="text-sm font-bold text-slate-900 mt-1">
                +{selectedCell.grossSpreadBps.toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-slate-400 mt-0.5">Top-of-book dislocation</div>
            </div>

            {/* Deductions: Taker Fees & Closing Friction */}
            <div className="bg-surface p-3 rounded-lg border border-surface-border">
              <div className="text-[10px] text-slate-500 uppercase tracking-wide">Deductions (Friction)</div>
              <div className="text-sm font-bold text-loss mt-1">
                -{(selectedCell.feeBps + selectedCell.closingFrictionBps).toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-slate-400 mt-0.5">
                Fees: {selectedCell.feeBps.toFixed(1)} | Exit: {selectedCell.closingFrictionBps.toFixed(1)} BPS
              </div>
            </div>

            {/* Net Spread */}
            <div className="bg-surface p-3 rounded-lg border border-surface-border">
              <div className="text-[10px] text-slate-500 uppercase tracking-wide">Net Spread</div>
              <div
                className={`text-sm font-bold mt-1 ${
                  selectedCell.netSpreadBps >= minBps
                    ? "text-profit"
                    : selectedCell.netSpreadBps > 0
                    ? "text-accent"
                    : "text-slate-500"
                }`}
              >
                {selectedCell.netSpreadBps > 0 ? "+" : ""}
                {selectedCell.netSpreadBps.toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-slate-400 mt-0.5">After all trading friction</div>
            </div>

            {/* Rolling Window Baseline */}
            <div className="bg-surface p-3 rounded-lg border border-surface-border">
              <div className="text-[10px] text-slate-500 uppercase tracking-wide">
                Rolling {windowOption} Baseline
              </div>
              <div className="text-sm font-bold text-slate-900 mt-1">
                {selectedCell.baselineMeanBps > 0 ? "+" : ""}
                {selectedCell.baselineMeanBps.toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-slate-500 mt-0.5">
                Δ vs Mean:{" "}
                <span className={selectedCell.deltaBps >= 0 ? "text-profit font-semibold" : "text-loss font-semibold"}>
                  {selectedCell.deltaBps > 0 ? "+" : ""}
                  {selectedCell.deltaBps.toFixed(2)} BPS
                </span>
              </div>
            </div>

            {/* Est Profit */}
            <div className="bg-surface p-3 rounded-lg border border-surface-border">
              <div className="text-[10px] text-slate-500 uppercase tracking-wide">Estimated Profit</div>
              <div
                className={`text-sm font-bold mt-1 ${
                  selectedCell.estProfitUsd >= 0 ? "text-profit" : "text-loss"
                }`}
              >
                {formatCurrencyPnl(selectedCell.estProfitUsd)}
              </div>
              <div className="text-[10px] text-slate-400 mt-0.5">
                {selectedCell.isExecutable ? "Target met (≥ min)" : "Sub-threshold"}
              </div>
            </div>
          </div>

          {/* Action Bar: Status & One-Click Execution Button */}
          <div className="flex flex-wrap items-center justify-between gap-3 pt-2 border-t border-surface-border">
            <div className="text-xs font-mono text-slate-600 flex items-center space-x-2">
              <Info className="w-3.5 h-3.5 text-accent inline" />
              <span>
                Atomic two-leg IOC orders dispatched concurrently with DeltaGuard slippage protection.
              </span>
            </div>

            <div className="flex items-center space-x-3">
              {executionResult && (
                <div
                  className={`text-xs font-mono px-3 py-1 rounded-lg border flex items-center space-x-1.5 ${
                    executionResult.success
                      ? "bg-profit-dim text-profit border-profit-border"
                      : "bg-loss-dim text-loss border-loss-border"
                  }`}
                >
                  {executionResult.success && <CheckCircle2 className="w-3.5 h-3.5" />}
                  <span>{executionResult.message}</span>
                </div>
              )}

              <button
                onClick={() => handleExecuteCellTrade(selectedCell)}
                disabled={executing || loading}
                className={`px-4 py-2 rounded-lg text-xs font-mono font-bold transition flex items-center space-x-2 shadow-xs ${
                  executing
                    ? "bg-slate-300 text-slate-500 cursor-not-allowed"
                    : selectedCell.isExecutable
                    ? "bg-accent hover:bg-accent-hover text-white cursor-pointer"
                    : "bg-surface-raised hover:bg-surface border border-surface-border text-slate-800 cursor-pointer"
                }`}
              >
                <Zap className={`w-3.5 h-3.5 ${executing ? "animate-spin" : ""}`} />
                <span>
                  {executing
                    ? "Routing IOC Orders..."
                    : `Execute Atomic Arbitrage (${selectedCell.rowLabel} ➔ ${selectedCell.colLabel})`}
                </span>
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Collapsible Ranked Arbitrage Opportunities List (Sorted Table) */}
      <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-xs">
        <div className="flex items-center justify-between mb-4">
          <div>
            <button
              onClick={() => setShowRankedList(!showRankedList)}
              className="text-sm font-bold font-mono text-slate-900 flex items-center space-x-2 hover:text-accent transition"
            >
              <span>Ranked Arbitrage Opportunities</span>
              <span className="px-2 py-0.5 rounded-full text-[10px] bg-accent-dim text-accent border border-accent-line font-medium">
                {opportunities.length} Active
              </span>
              {showRankedList ? <ChevronUp className="w-4 h-4 text-slate-500" /> : <ChevronDown className="w-4 h-4 text-slate-500" />}
            </button>
            <p className="text-xs text-slate-500 mt-0.5">
              Instantaneous cross-venue sorted spread rankings for {symbol}.
            </p>
          </div>
        </div>

        {showRankedList && (
          opportunities.length === 0 ? (
            <div className="text-center py-8 text-slate-400 font-mono text-xs border border-dashed border-surface-border rounded-lg">
              No spatial price discrepancies currently exceeding {minBps} BPS threshold.
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-left text-xs font-mono">
                <thead>
                  <tr className="border-b border-surface-border text-slate-500 uppercase text-[10px] tracking-wider">
                    <th className="py-2.5 px-3">Route (Buy / Sell)</th>
                    <th className="py-2.5 px-3">Buy Price</th>
                    <th className="py-2.5 px-3">Sell Price</th>
                    <th className="py-2.5 px-3">Gross Spread</th>
                    <th className="py-2.5 px-3">Roundtrip Fees</th>
                    <th className="py-2.5 px-3">Net Spread</th>
                    <th className="py-2.5 px-3">Est. Profit</th>
                    <th className="py-2.5 px-3 text-right">Action</th>
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
                        <td className="py-3 px-3 font-medium text-slate-900 flex items-center space-x-1.5">
                          <span className="text-slate-800">{opp.buy_venue}</span>
                          <span className="text-slate-400 text-xs">/</span>
                          <span className="text-slate-800">{opp.sell_venue}</span>
                        </td>
                        <td className="py-3 px-3 text-loss font-medium">{formatCurrencyPrice(opp.buy_price)}</td>
                        <td className="py-3 px-3 text-profit font-medium">{formatCurrencyPrice(opp.sell_price)}</td>
                        <td className="py-3 px-3 text-slate-700">
                          {opp.gross_spread_bps.toFixed(1)} BPS ({formatSpreadPrice(opp.gross_spread)})
                        </td>
                        <td className="py-3 px-3 text-slate-500">{opp.fee_bps.toFixed(1)} BPS</td>
                        <td className="py-3 px-3">
                          <span
                            className={`font-bold ${
                              opp.net_spread_bps >= minBps ? "text-profit" : "text-slate-500"
                            }`}
                          >
                            {opp.net_spread_bps.toFixed(2)} BPS
                          </span>
                        </td>
                        <td
                          className={`py-3 px-3 font-bold ${
                            opp.estimated_profit_usd >= 0 ? "text-profit" : "text-loss"
                          }`}
                        >
                          {formatCurrencyPnl(opp.estimated_profit_usd)}
                        </td>
                        <td className="py-3 px-3 text-right">
                          <button
                            onClick={() => onExecuteTrade(opp)}
                            disabled={!isExec || loading}
                            className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition flex items-center ml-auto shadow-xs ${
                              isExec
                                ? "bg-accent hover:bg-accent-hover text-white"
                                : "bg-surface-raised text-slate-400 cursor-not-allowed border border-surface-border"
                            }`}
                          >
                            <span>Execute</span>
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )
        )}
      </div>
    </div>
  );
};
