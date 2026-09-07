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

  return (
    <div className="p-6 space-y-4 overflow-y-auto h-full text-white/95">
      {/* Title & Automated Execution Header Banner */}
      <div className="glass-panel p-4 rounded-2xl flex flex-col md:flex-row md:items-center md:justify-between gap-4">
        <div>
          <div className="flex items-center space-x-2">
            <span className="p-1.5 bg-cyan-500/10 text-cyan-300 rounded-lg border border-cyan-500/20">
              <Layers className="w-5 h-5 text-cyan-400" />
            </span>
            <h1 className="text-base font-bold text-white tracking-tight font-sans">
              Futures Spread Convergence Matrix
            </h1>
            <span className="px-2 py-0.5 rounded text-[11px] font-mono font-semibold bg-cyan-500/10 text-cyan-300 border border-cyan-500/25">
              Auto-Execution Basis Arb
            </span>
          </div>
          <p className="text-xs text-white/50 font-sans mt-1">
            Automated cross-venue perpetual basis spread convergence engine. Continuously scans live orderbooks and auto-executes atomic legs whenever Net Spread ≥ Hurdle.
          </p>
        </div>

        {/* Auto-Execution Engine Control */}
        <div className="flex items-center space-x-3 shrink-0">
          <div className="flex items-center space-x-2.5 bg-white/[0.04] px-3.5 py-1.5 rounded-xl border border-white/[0.08]">
            <span
              className={`w-2 h-2 rounded-full ${
                autoExecute ? "bg-emerald-400 animate-pulse shadow-[0_0_8px_rgba(52,211,153,0.8)]" : "bg-white/30"
              }`}
            />
            <span className="text-xs font-mono font-bold text-white/80">
              {autoExecute ? "AUTO-BOT: ACTIVE" : "AUTO-BOT: PAUSED"}
            </span>
            <button
              onClick={() => setAutoExecute((prev) => !prev)}
              className={`ml-2 px-2.5 py-1 rounded-md text-[11px] font-mono font-bold transition ${
                autoExecute
                  ? "bg-emerald-500/20 text-emerald-300 border border-emerald-500/40 hover:bg-emerald-500/30 shadow-[0_0_12px_rgba(52,211,153,0.2)]"
                  : "bg-white/[0.06] border border-white/[0.1] text-white/70 hover:text-white"
              }`}
            >
              {autoExecute ? "Pause Auto-Bot" : "Enable Auto-Bot"}
            </button>
          </div>
        </div>
      </div>

      {lastAutoExecMsg && (
        <div className="px-4 py-2 bg-emerald-500/15 border border-emerald-500/30 rounded-xl text-xs font-mono text-emerald-300 flex items-center justify-between shadow-[0_0_12px_rgba(52,211,153,0.15)]">
          <span>{lastAutoExecMsg}</span>
          <button onClick={() => setLastAutoExecMsg(null)} className="text-emerald-400 hover:text-white text-xs font-bold">×</button>
        </div>
      )}

      {/* Top Bar: Mode Selector & Quant Controls */}
      <div className="flex flex-wrap items-center justify-between gap-4 glass-panel p-4 rounded-xl">
        {/* Left Side: Mode Toggle & Active Ticker */}
        <div className="flex flex-wrap items-center gap-3">
          {/* Matrix Mode Toggle */}
          <div className="flex items-center bg-white/[0.03] p-1 rounded-lg border border-white/[0.08]">
            <button
              onClick={() => {
                setMatrixMode("cross-venue");
                setSelectedPair(null);
              }}
              className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-xs font-mono font-medium transition ${
                matrixMode === "cross-venue"
                  ? "bg-white/[0.12] text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.2)] border border-cyan-500/30"
                  : "text-white/50 hover:text-white/90"
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
                  ? "bg-white/[0.12] text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.2)] border border-cyan-500/30"
                  : "text-white/50 hover:text-white/90"
              }`}
            >
              <Layers className="w-3.5 h-3.5" />
              <span>Cross-Ticker Matrix</span>
            </button>
          </div>

          {/* Active Ticker Badge */}
          <div className="text-xs font-mono font-bold text-white bg-white/[0.04] px-2.5 py-1.5 rounded-md border border-white/[0.08] flex items-center space-x-2">
            <span>{symbol}</span>
            <span className="flex space-x-1">
              {activeTicker.venues.map((v) => (
                <span
                  key={v}
                  className="text-[9px] px-1.5 py-0.2 rounded font-semibold bg-white/[0.06] text-cyan-300 border border-cyan-500/20"
                >
                  {v}
                </span>
              ))}
            </span>
            <span
              className="text-[10px] px-1.5 py-0.2 rounded bg-white/[0.04] border border-white/[0.08] font-medium text-white/70 tabular-nums"
              title={`${tickerStats.count} strategy executions`}
            >
              {tickerStats.count} tx{tickerStats.count === 1 ? "" : "s"}
            </span>
            <span
              className={`text-[10px] px-1.5 py-0.2 rounded font-semibold tabular-nums ${
                tickerStats.pnl > 0
                  ? "text-emerald-400 bg-emerald-500/10 border border-emerald-500/20"
                  : tickerStats.pnl < 0
                  ? "text-rose-400 bg-rose-500/10 border border-rose-500/20"
                  : "text-white/50 bg-white/[0.04] border border-white/[0.08]"
              }`}
              title={`Strategy Net PnL: ${formatCurrencyPnl(tickerStats.pnl)}`}
            >
              {formatCurrencyPnl(tickerStats.pnl)}
            </span>
          </div>

          {/* Cross-Ticker Presets (only shown in Cross-Ticker mode) */}
          {matrixMode === "cross-ticker" && (
            <div className="flex items-center space-x-1.5 bg-white/[0.04] px-2 py-1 rounded-md border border-white/[0.08] text-xs font-mono">
              <span className="text-white/40 text-[11px]">Preset:</span>
              <select
                value={selectedPreset}
                onChange={(e) => {
                  setSelectedPreset(e.target.value);
                  setSelectedPair(null);
                }}
                className="bg-[#0C1019] border border-white/[0.1] rounded px-2 py-0.5 text-xs font-mono text-white/90 focus:outline-none focus:ring-1 focus:ring-cyan-400"
              >
                {Object.keys(TICKER_PRESETS).map((p) => (
                  <option key={p} value={p} className="bg-[#0C1019] text-white">
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
          <div className="flex items-center space-x-1 bg-white/[0.03] p-1 rounded-lg border border-white/[0.08] text-xs font-mono">
            <span className="text-white/40 text-[10px] uppercase font-semibold px-1.5">Window:</span>
            {(["10s", "30s", "1m", "5m", "15m"] as SlidingWindowOption[]).map((w) => (
              <button
                key={w}
                onClick={() => setWindowOption(w)}
                className={`px-2 py-1 rounded text-xs font-mono font-medium transition ${
                  windowOption === w
                    ? "bg-indigo-500/30 text-indigo-200 border border-indigo-500/50 shadow-[0_0_12px_rgba(129,140,248,0.3)]"
                    : "text-white/50 hover:text-white/90 hover:bg-white/[0.04]"
                }`}
                title={`Track baseline over rolling ${w}`}
              >
                {w}
              </button>
            ))}
          </div>

          {/* Benchmark Order Qty */}
          <div className="flex items-center space-x-1.5 text-xs font-mono bg-white/[0.04] px-2.5 py-1 rounded-md border border-white/[0.08]">
            <span className="text-white/40">Qty:</span>
            <input
              type="number"
              step={orderQty >= 1000 ? "100" : orderQty >= 10 ? "1" : orderQty >= 1 ? "0.1" : "0.001"}
              min="0.00000001"
              value={orderQty}
              onChange={(e) => setOrderQty(parseFloat(e.target.value) || 0.1)}
              className="w-20 px-1.5 py-0.5 bg-white/[0.06] border border-white/[0.1] rounded text-white font-mono text-xs tabular-nums focus:outline-none focus:ring-1 focus:ring-cyan-400"
            />
          </div>

          {/* Min Net BPS Filter */}
          <div className="flex items-center space-x-1.5 text-xs font-mono bg-white/[0.04] px-2.5 py-1 rounded-md border border-white/[0.08]">
            <span className="text-white/40">Min BPS:</span>
            <input
              type="number"
              step="1"
              value={minBps}
              onChange={(e) => setMinBps(parseFloat(e.target.value) || 0)}
              className="w-14 px-1.5 py-0.5 bg-white/[0.06] border border-white/[0.1] rounded text-white font-mono text-xs tabular-nums focus:outline-none focus:ring-1 focus:ring-cyan-400"
            />
          </div>

          {/* Refresh Button */}
          <button
            onClick={() => onRefresh(orderQty, minBps)}
            disabled={loading}
            className="flex items-center space-x-1.5 px-3 py-1.5 bg-white/[0.06] hover:bg-white/[0.1] border border-white/[0.1] rounded-lg text-xs font-mono text-white/90 font-medium transition shadow-xs"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-cyan-400" : "text-white/50"}`} />
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
            <h3 className="text-xs font-mono uppercase tracking-wider text-white/40 font-semibold">
              Venue Market Quotes (Top of Book)
            </h3>
            <span className="text-[11px] font-mono text-white/40">Target: {symbol}</span>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
            {orderbooks.map((book) => {
              const spreadColor = book.spread_bps < 10 ? "text-emerald-400" : "text-amber-400";
              const topBidSize = book.bids.length > 0 ? book.bids[0][1] : 0;
              const topAskSize = book.asks.length > 0 ? book.asks[0][1] : 0;

              return (
                <div
                  key={book.venue_id}
                  className="glass-panel rounded-xl p-4 flex flex-col justify-between hover:border-white/[0.18] transition"
                >
                  <div className="flex items-center justify-between border-b border-white/[0.08] pb-2.5 mb-3">
                    <div>
                      <span className="font-mono text-xs font-bold text-white/95">{book.venue_name}</span>
                      <div className="text-[10px] font-mono text-white/40 mt-0.5 tabular-nums">
                        8h Funding: {book.funding_rate_8h_pct.toFixed(4)}%
                      </div>
                    </div>
                    <div className="text-right">
                      <span className="text-[10px] font-mono text-white/40">Spread:</span>{" "}
                      <span className={`font-mono text-xs font-semibold tabular-nums ${spreadColor}`}>
                        {book.spread_bps.toFixed(1)} BPS
                      </span>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 gap-2 bg-white/[0.03] p-2.5 rounded-lg border border-white/[0.06] mb-3">
                    <div>
                      <div className="text-[10px] font-mono text-white/40 uppercase tracking-wide flex justify-between">
                        <span>Bid (Buy)</span>
                        <span className="text-white/40 tabular-nums">{topBidSize > 0 ? topBidSize.toFixed(2) : ""}</span>
                      </div>
                      <div className="text-sm font-mono font-bold text-emerald-400 mt-1 tabular-nums">
                        {formatCurrencyPrice(book.best_bid)}
                      </div>
                    </div>

                    <div className="border-l border-white/[0.08] pl-2.5">
                      <div className="text-[10px] font-mono text-white/40 uppercase tracking-wide flex justify-between">
                        <span>Ask (Sell)</span>
                        <span className="text-white/40 tabular-nums">{topAskSize > 0 ? topAskSize.toFixed(2) : ""}</span>
                      </div>
                      <div className="text-sm font-mono font-bold text-rose-400 mt-1 tabular-nums">
                        {formatCurrencyPrice(book.best_ask)}
                      </div>
                    </div>
                  </div>

                  <div className="pt-2 border-t border-white/[0.06] text-[11px] font-mono text-white/40 flex justify-between items-center">
                    <span>Mid Price</span>
                    <span className="font-semibold text-white/80 tabular-nums">{formatCurrencyPrice(book.mid_price)}</span>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Real 2D Interactive Matrix Grid */}
      {/* Real 2D Interactive Matrix Grid */}
      <div className="glass-panel p-5 space-y-4 rounded-2xl">
        {/* Header with Title and Quant Legend */}
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-white/[0.08] pb-3">
          <div>
            <h3 className="text-sm font-bold font-mono text-white flex items-center space-x-2">
              <span>
                {matrixMode === "cross-venue"
                  ? `Arbitrage Dislocation Matrix — [Cross-Venue: ${symbol}]`
                  : `Cross-Ticker Relative Value Matrix — [Preset: ${selectedPreset}]`}
              </span>
              <span className="px-2 py-0.5 rounded-full text-[10px] bg-indigo-500/20 text-indigo-300 border border-indigo-500/30 font-medium">
                Rolling {windowOption} Window
              </span>
            </h3>
            <p className="text-xs text-white/50 mt-1">
              Y-Axis = <strong className="text-white/80">LONG (Buy Leg)</strong> · X-Axis = <strong className="text-white/80">SHORT (Sell Leg)</strong>. Cell shows{" "}
              <strong className="text-cyan-300">Net BPS</strong> after taker fees &amp; closing spread friction, and <strong className="text-white/80">Δ BPS</strong> vs rolling {windowOption} baseline.
            </p>
          </div>

          {/* Color Legend */}
          <div className="flex items-center space-x-3 text-[11px] font-mono">
            <div className="flex items-center space-x-1.5">
              <span className="w-2.5 h-2.5 rounded bg-emerald-500/20 border border-emerald-400 inline-block shadow-[0_0_8px_rgba(52,211,153,0.5)]"></span>
              <span className="text-emerald-300 font-medium">Net ≥ {minBps} BPS (Executable)</span>
            </div>
            <div className="flex items-center space-x-1.5">
              <span className="w-2.5 h-2.5 rounded bg-cyan-500/15 border border-cyan-400 inline-block"></span>
              <span className="text-cyan-300 font-medium">0 &lt; Net &lt; {minBps} BPS</span>
            </div>
            <div className="flex items-center space-x-1.5">
              <span className="w-2.5 h-2.5 rounded bg-white/[0.04] border border-white/[0.1] inline-block"></span>
              <span className="text-white/50">Net ≤ 0</span>
            </div>
            <div className="flex items-center space-x-1.5">
              <span className="w-2.5 h-2.5 rounded bg-white/[0.02] border border-white/[0.05] inline-block"></span>
              <span className="text-white/30">— (Diagonal)</span>
            </div>
          </div>
        </div>

        {/* The 2D Interactive Matrix Table */}
        <div className="overflow-auto max-h-[640px] rounded-xl border border-white/[0.08]">
          {matrixMode === "cross-venue" ? (
            /* Cross-Venue Matrix Grid */
            venueMatrix.grid.length === 0 ? (
              <div className="py-12 text-center text-white/40 font-mono text-xs">
                Scanning orderbooks for {symbol}...
              </div>
            ) : (
              <table className="w-full border-collapse font-mono text-xs tabular-nums">
                <thead className="sticky top-0 z-20 bg-[#0C1019] shadow-xs">
                  <tr className="bg-[#0C1019] border-b border-white/[0.08]">
                    <th className="p-3 text-left font-semibold text-white/40 uppercase text-[10px] tracking-wider border-r border-b border-white/[0.08] min-w-[140px] sticky top-0 left-0 bg-[#0C1019] z-30">
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
                          className="p-3 text-center font-bold text-white/90 border-r border-b border-white/[0.08] last:border-r-0 min-w-[130px] sticky top-0 bg-[#0C1019] z-20"
                        >
                          <div className="text-[11px] text-white/95">{colVenue}</div>
                          <div className="text-[10px] font-normal text-emerald-400 mt-0.5 tabular-nums">
                            Sell @ {formatCurrencyPrice(sellPrice)}
                          </div>
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody className="divide-y divide-white/[0.06]">
                  {venueMatrix.grid.map((row, i) => {
                    const rowLabel = venueMatrix.venueLabels[i];
                    const buyPrice = orderbooks[i]?.best_ask ?? 0;

                    return (
                      <tr key={venueMatrix.venues[i]} className="hover:bg-white/[0.02] transition">
                        {/* Row Header (Long Leg) */}
                        <td className="p-3 font-semibold text-white/95 border-r border-white/[0.08] bg-[#0C1019]/95 sticky left-0 z-10">
                          <div className="text-[11px] font-bold text-white/95">{rowLabel}</div>
                          <div className="text-[10px] font-normal text-rose-400 mt-0.5 tabular-nums">
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
                                className="p-3 text-center border-r border-white/[0.06] last:border-r-0 bg-white/[0.02] select-none"
                              >
                                <span className="text-white/20 font-mono text-base">—</span>
                              </td>
                            );
                          }

                          return (
                            <td
                              key={`${cell.rowId}-${cell.colId}`}
                              onClick={() => setSelectedPair({ rowId: cell.rowId, colId: cell.colId })}
                              className={`p-2.5 text-center border-r border-white/[0.06] last:border-r-0 cursor-pointer transition select-none ${
                                visuals.bgClass
                              } ${isSelected ? "ring-2 ring-cyan-400 shadow-[0_0_16px_rgba(34,211,238,0.4)]" : ""}`}
                              title={`Click to inspect & execute: Long ${cell.rowLabel} / Short ${cell.colLabel}`}
                            >
                              <div className="flex flex-col items-center justify-center space-y-1">
                                {/* Net Spread BPS */}
                                <div className={`text-xs font-mono font-bold tabular-nums ${visuals.textClass}`}>
                                  {cell.netSpreadBps > 0 ? "+" : ""}
                                  {cell.netSpreadBps.toFixed(1)} BPS
                                </div>

                                {/* Window Difference Δ BPS */}
                                <div
                                  className={`flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] font-mono tabular-nums ${visuals.deltaBadgeBg}`}
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
              <div className="py-12 text-center text-white/40 font-mono text-xs">
                Building multi-market matrix for {selectedPreset}...
              </div>
            ) : (
              <table className="w-full border-collapse font-mono text-xs tabular-nums">
                <thead className="sticky top-0 z-20 bg-[#0C1019] shadow-xs">
                  <tr className="bg-[#0C1019] border-b border-white/[0.08]">
                    <th className="p-3 text-left font-semibold text-white/40 uppercase text-[10px] tracking-wider border-r border-b border-white/[0.08] min-w-[140px] sticky top-0 left-0 bg-[#0C1019] z-30">
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
                          className="p-3 text-center font-bold text-white/90 border-r border-b border-white/[0.08] last:border-r-0 min-w-[124px] sticky top-0 bg-[#0C1019] z-20"
                        >
                          <div className="text-[11px] text-white/95">{colSymbol}</div>
                          <div className="text-[10px] font-normal text-emerald-400 mt-0.5 tabular-nums">
                            Bid: {quote ? formatCurrencyPrice(quote.bestBid) : "..."}
                          </div>
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody className="divide-y divide-white/[0.06]">
                  {crossTickerMatrix.grid.map((row, i) => {
                    const rowLabel = crossTickerMatrix.symbolLabels[i];
                    const rowSym = crossTickerMatrix.symbols[i];
                    const quote = tickerQuotesMap.get(rowSym);

                    return (
                      <tr key={rowSym} className="hover:bg-white/[0.02] transition">
                        {/* Row Header (Long Leg) */}
                        <td className="p-3 font-semibold text-white/95 border-r border-white/[0.08] bg-[#0C1019]/95 sticky left-0 z-10">
                          <div className="text-[11px] font-bold text-white/95">{rowLabel}</div>
                          <div className="text-[10px] font-normal text-rose-400 mt-0.5 tabular-nums">
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
                                className="p-3 text-center border-r border-white/[0.06] last:border-r-0 bg-white/[0.02] select-none"
                              >
                                <span className="text-white/20 font-mono text-base">—</span>
                              </td>
                            );
                          }

                          return (
                            <td
                              key={`${cell.rowId}-${cell.colId}`}
                              onClick={() => setSelectedPair({ rowId: cell.rowId, colId: cell.colId })}
                              className={`p-2.5 text-center border-r border-white/[0.06] last:border-r-0 cursor-pointer transition select-none ${
                                visuals.bgClass
                              } ${isSelected ? "ring-2 ring-cyan-400 shadow-[0_0_16px_rgba(34,211,238,0.4)]" : ""}`}
                              title={`Click to inspect & execute: Long ${cell.rowLabel} / Short ${cell.colLabel}`}
                            >
                              <div className="flex flex-col items-center justify-center space-y-1">
                                <div className={`text-xs font-mono font-bold tabular-nums ${visuals.textClass}`}>
                                  {cell.netSpreadBps > 0 ? "+" : ""}
                                  {cell.netSpreadBps.toFixed(1)} BPS
                                </div>

                                <div
                                  className={`flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] font-mono tabular-nums ${visuals.deltaBadgeBg}`}
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
        <div className="glass-panel-raised border border-cyan-500/30 p-5 rounded-2xl shadow-[0_0_30px_rgba(34,211,238,0.1)] space-y-4">
          <div className="flex items-center justify-between border-b border-white/[0.08] pb-3">
            <div className="flex items-center space-x-2">
              <Zap className="w-4 h-4 text-cyan-400" />
              <h4 className="text-sm font-bold font-mono text-white">
                Atomic Execution &amp; Spread Analytics
              </h4>
              <span className="text-xs px-2 py-0.5 rounded font-mono font-semibold bg-white/[0.04] border border-white/[0.08] text-white/70">
                {matrixMode === "cross-venue" ? "Cross-Venue Spatial Arbitrage" : "Cross-Ticker Statistical Relative Value"}
              </span>
            </div>
            <button
              onClick={() => setSelectedPair(null)}
              className="text-white/40 hover:text-white p-1 rounded-md transition"
              title="Close panel"
            >
              <X className="w-4 h-4" />
            </button>
          </div>

          {/* Trade Route Banner */}
          <div className="flex flex-wrap items-center justify-between gap-4 bg-white/[0.03] p-3.5 rounded-xl border border-white/[0.06] font-mono text-xs">
            <div className="flex items-center space-x-3">
              <div className="flex items-center space-x-1.5">
                <span className="px-2 py-0.5 rounded bg-rose-500/15 text-rose-300 font-bold border border-rose-500/30">
                  BUY / LONG
                </span>
                <span className="font-bold text-white">{selectedCell.rowLabel}</span>
                <span className="text-white/40">({selectedCell.longVenue})</span>
                <span className="font-semibold text-rose-400 tabular-nums">@{formatCurrencyPrice(selectedCell.longPrice)}</span>
              </div>

              <ArrowRight className="w-4 h-4 text-white/30" />

              <div className="flex items-center space-x-1.5">
                <span className="px-2 py-0.5 rounded bg-emerald-500/15 text-emerald-300 font-bold border border-emerald-500/30">
                  SELL / SHORT
                </span>
                <span className="font-bold text-white">{selectedCell.colLabel}</span>
                <span className="text-white/40">({selectedCell.shortVenue})</span>
                <span className="font-semibold text-emerald-400 tabular-nums">@{formatCurrencyPrice(selectedCell.shortPrice)}</span>
              </div>
            </div>

            <div className="text-right">
              <span className="text-white/40">Order Quantity: </span>
              <span className="font-bold text-white tabular-nums">
                {selectedCell.orderQty} {selectedCell.longSymbol.split("-")[0]}
              </span>
            </div>
          </div>

          {/* Quant Breakdown Metric Cards */}
          <div className="grid grid-cols-2 md:grid-cols-5 gap-3 font-mono">
            {/* Gross Spread */}
            <div className="bg-white/[0.04] p-3.5 rounded-xl border border-white/[0.08]">
              <div className="text-[10px] text-white/40 uppercase tracking-wide">Gross Spread</div>
              <div className="text-sm font-bold text-white mt-1 tabular-nums">
                +{selectedCell.grossSpreadBps.toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-white/40 mt-0.5">Top-of-book dislocation</div>
            </div>

            {/* Deductions: Taker Fees & Closing Friction */}
            <div className="bg-white/[0.04] p-3.5 rounded-xl border border-white/[0.08]">
              <div className="text-[10px] text-white/40 uppercase tracking-wide">Deductions (Friction)</div>
              <div className="text-sm font-bold text-rose-400 mt-1 tabular-nums">
                -{(selectedCell.feeBps + selectedCell.closingFrictionBps).toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-white/40 mt-0.5 tabular-nums">
                Fees: {selectedCell.feeBps.toFixed(1)} | Exit: {selectedCell.closingFrictionBps.toFixed(1)} BPS
              </div>
            </div>

            {/* Net Spread */}
            <div className="bg-white/[0.04] p-3.5 rounded-xl border border-white/[0.08]">
              <div className="text-[10px] text-white/40 uppercase tracking-wide">Net Spread</div>
              <div
                className={`text-sm font-bold mt-1 tabular-nums ${
                  selectedCell.netSpreadBps >= minBps
                    ? "text-emerald-400"
                    : selectedCell.netSpreadBps > 0
                    ? "text-cyan-300"
                    : "text-white/50"
                }`}
              >
                {selectedCell.netSpreadBps > 0 ? "+" : ""}
                {selectedCell.netSpreadBps.toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-white/40 mt-0.5">After all trading friction</div>
            </div>

            {/* Rolling Window Baseline */}
            <div className="bg-white/[0.04] p-3.5 rounded-xl border border-white/[0.08]">
              <div className="text-[10px] text-white/40 uppercase tracking-wide">
                Rolling {windowOption} Baseline
              </div>
              <div className="text-sm font-bold text-white mt-1 tabular-nums">
                {selectedCell.baselineMeanBps > 0 ? "+" : ""}
                {selectedCell.baselineMeanBps.toFixed(2)} BPS
              </div>
              <div className="text-[10px] text-white/50 mt-0.5 tabular-nums">
                Δ vs Mean:{" "}
                <span className={selectedCell.deltaBps >= 0 ? "text-emerald-400 font-semibold" : "text-rose-400 font-semibold"}>
                  {selectedCell.deltaBps > 0 ? "+" : ""}
                  {selectedCell.deltaBps.toFixed(2)} BPS
                </span>
              </div>
            </div>

            {/* Est Profit */}
            <div className="bg-white/[0.04] p-3.5 rounded-xl border border-white/[0.08]">
              <div className="text-[10px] text-white/40 uppercase tracking-wide">Estimated Profit</div>
              <div
                className={`text-sm font-bold mt-1 tabular-nums ${
                  selectedCell.estProfitUsd >= 0 ? "text-emerald-400" : "text-rose-400"
                }`}
              >
                {formatCurrencyPnl(selectedCell.estProfitUsd)}
              </div>
              <div className="text-[10px] text-white/40 mt-0.5">
                {selectedCell.isExecutable ? "Target met (≥ min)" : "Sub-threshold"}
              </div>
            </div>
          </div>

          {/* Action Bar: Status & One-Click Execution Button */}
          <div className="flex flex-wrap items-center justify-between gap-3 pt-2 border-t border-white/[0.08]">
            <div className="text-xs font-mono text-white/50 flex items-center space-x-2">
              <Info className="w-3.5 h-3.5 text-cyan-400 inline" />
              <span>
                Atomic two-leg IOC orders dispatched concurrently with DeltaGuard slippage protection.
              </span>
            </div>

            <div className="flex items-center space-x-3">
              {executionResult && (
                <div
                  className={`text-xs font-mono px-3 py-1.5 rounded-lg border flex items-center space-x-1.5 ${
                    executionResult.success
                      ? "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"
                      : "bg-rose-500/15 text-rose-300 border-rose-500/30"
                  }`}
                >
                  {executionResult.success && <CheckCircle2 className="w-3.5 h-3.5" />}
                  <span>{executionResult.message}</span>
                </div>
              )}

              <button
                onClick={() => handleExecuteCellTrade(selectedCell)}
                disabled={executing || loading}
                className={`px-5 py-2.5 rounded-xl text-xs font-mono font-bold transition flex items-center space-x-2 ${
                  executing
                    ? "bg-white/20 text-white/40 cursor-not-allowed"
                    : selectedCell.isExecutable
                    ? "bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white shadow-[0_0_20px_rgba(34,211,238,0.35)] cursor-pointer"
                    : "bg-white/[0.06] hover:bg-white/[0.1] border border-white/[0.1] text-white/80 cursor-pointer"
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
      <div className="glass-panel rounded-2xl p-5 space-y-4">
        <div className="flex items-center justify-between mb-2">
          <div>
            <button
              onClick={() => setShowRankedList(!showRankedList)}
              className="text-sm font-bold font-mono text-white flex items-center space-x-2 hover:text-cyan-300 transition"
            >
              <span>Ranked Arbitrage Opportunities</span>
              <span className="px-2 py-0.5 rounded-full text-[10px] bg-cyan-500/15 text-cyan-300 border border-cyan-500/30 font-medium tabular-nums">
                {opportunities.length} Active
              </span>
              {showRankedList ? <ChevronUp className="w-4 h-4 text-white/40" /> : <ChevronDown className="w-4 h-4 text-white/40" />}
            </button>
            <p className="text-xs text-white/40 mt-0.5">
              Instantaneous cross-venue sorted spread rankings for {symbol}.
            </p>
          </div>
        </div>

        {showRankedList && (
          opportunities.length === 0 ? (
            <div className="text-center py-8 text-white/40 font-mono text-xs border border-dashed border-white/[0.08] rounded-xl">
              No spatial price discrepancies currently exceeding {minBps} BPS threshold.
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-left text-xs font-mono tabular-nums">
                <thead>
                  <tr className="border-b border-white/[0.08] text-white/40 uppercase text-[10px] tracking-wider">
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
                <tbody className="divide-y divide-white/[0.06]">
                  {opportunities.map((opp) => {
                    const isExec = opp.is_executable;
                    return (
                      <tr
                        key={opp.id}
                        className={`hover:bg-white/[0.04] transition ${
                          isExec ? "bg-cyan-500/[0.04]" : ""
                        }`}
                      >
                        <td className="py-3 px-3 font-medium text-white flex items-center space-x-1.5">
                          <span className="text-white/90">{opp.buy_venue}</span>
                          <span className="text-white/30 text-xs">/</span>
                          <span className="text-white/90">{opp.sell_venue}</span>
                        </td>
                        <td className="py-3 px-3 text-rose-400 font-medium tabular-nums">{formatCurrencyPrice(opp.buy_price)}</td>
                        <td className="py-3 px-3 text-emerald-400 font-medium tabular-nums">{formatCurrencyPrice(opp.sell_price)}</td>
                        <td className="py-3 px-3 text-white/70 tabular-nums">
                          {opp.gross_spread_bps.toFixed(1)} BPS ({formatSpreadPrice(opp.gross_spread)})
                        </td>
                        <td className="py-3 px-3 text-white/40 tabular-nums">{opp.fee_bps.toFixed(1)} BPS</td>
                        <td className="py-3 px-3">
                          <span
                            className={`font-bold tabular-nums ${
                              opp.net_spread_bps >= minBps ? "text-emerald-400" : "text-white/50"
                            }`}
                          >
                            {opp.net_spread_bps.toFixed(2)} BPS
                          </span>
                        </td>
                        <td
                          className={`py-3 px-3 font-bold tabular-nums ${
                            opp.estimated_profit_usd >= 0 ? "text-emerald-400" : "text-rose-400"
                          }`}
                        >
                          {formatCurrencyPnl(opp.estimated_profit_usd)}
                        </td>
                        <td className="py-3 px-3 text-right">
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
                            className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition flex items-center ml-auto shadow-xs ${
                              isExec
                                ? "bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white shadow-[0_0_12px_rgba(34,211,238,0.25)]"
                                : "bg-white/[0.04] text-white/30 cursor-not-allowed border border-white/[0.06]"
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

      {/* Transaction History & Execution Telemetry for Active Ticker */}
      <TickerTransactionHistory
        symbol={symbol}
        logs={executionLogs || []}
        config={currentStrategyConfig}
        onRefresh={() => onRefresh(orderQty, minBps)}
      />
    </div>
  );
};
