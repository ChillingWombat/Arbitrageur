import React, { useState, useMemo } from "react";
import { Search, X, ArrowDownAZ, Activity, DollarSign } from "lucide-react";
import {
  PERP_TICKERS,
  formatCurrencyPrice,
  formatCurrencyPnl,
  type TickerSortOption,
  type TickerWithStats,
} from "../types/tickers";
import type { ExecutionLogEntry, TickerStrategyConfigDto } from "../types";

interface TickerExplorerProps {
  selectedSymbol: string;
  onSelectTicker: (symbol: string) => void;
  executionLogs: ExecutionLogEntry[];
  strategyConfigs?: Map<string, TickerStrategyConfigDto>;
}

export const TickerExplorer: React.FC<TickerExplorerProps> = ({
  selectedSymbol,
  onSelectTicker,
  executionLogs,
  strategyConfigs,
}) => {
  const [searchQuery, setSearchQuery] = useState("");
  const [sortOption, setSortOption] = useState<TickerSortOption>("alpha");

  // Compute stats (transaction count and net profit) per ticker from execution logs
  const tickersWithStats: TickerWithStats[] = useMemo(() => {
    const statsMap = new Map<string, { count: number; pnl: number; lastTime?: string }>();

    for (const log of executionLogs) {
      if (!log.symbol) continue;
      let baseKey = log.symbol.toUpperCase().split(/[-_/]/)[0];
      if (baseKey.startsWith("1000") && baseKey.length > 4) {
        baseKey = baseKey.slice(4);
      } else if (baseKey.startsWith("K") && baseKey.length > 3) {
        baseKey = baseKey.slice(1);
      }

      const cur = statsMap.get(baseKey) || { count: 0, pnl: 0 };
      cur.count += 1;
      cur.pnl += log.net_profit || 0;
      if (!cur.lastTime || log.timestamp > cur.lastTime) {
        cur.lastTime = log.timestamp;
      }
      statsMap.set(baseKey, cur);
    }

    return PERP_TICKERS.map((t) => {
      const stat = statsMap.get(t.base.toUpperCase()) || { count: 0, pnl: 0 };
      return {
        ...t,
        tradeCount: stat.count,
        netProfit: Math.round(stat.pnl * 100) / 100,
        lastTradeTime: stat.lastTime,
      };
    });
  }, [executionLogs]);

  // Filter and sort the tickers
  const processedTickers = useMemo(() => {
    // 1. Filter by query
    let list = tickersWithStats;
    const query = searchQuery.trim().toLowerCase();
    if (query) {
      list = list.filter(
        (t) =>
          t.symbol.toLowerCase().includes(query) ||
          t.base.toLowerCase().includes(query) ||
          t.name.toLowerCase().includes(query) ||
          t.quote.toLowerCase().includes(query) ||
          query === "perp" ||
          t.venues.some((v) => v.toLowerCase().includes(query))
      );
    }

    // 2. Sort
    return [...list].sort((a, b) => {
      if (sortOption === "alpha") {
        // Alphabetical A-Z (default)
        return a.symbol.localeCompare(b.symbol);
      }

      if (sortOption === "trades") {
        // Strategy transaction count descending (top 1 has highest count)
        if (b.tradeCount !== a.tradeCount) {
          return b.tradeCount - a.tradeCount;
        }
        // Tie break by net profit descending, then alphabetical
        if (b.netProfit !== a.netProfit) {
          return b.netProfit - a.netProfit;
        }
        return a.symbol.localeCompare(b.symbol);
      }

      if (sortOption === "pnl") {
        // Total net profit: positive first (descending), then negative (descending), then zero
        const isPosA = a.netProfit > 0.00001;
        const isPosB = b.netProfit > 0.00001;
        const isNegA = a.netProfit < -0.00001;
        const isNegB = b.netProfit < -0.00001;
        const isZeroA = !isPosA && !isNegA;
        const isZeroB = !isPosB && !isNegB;

        // Group 1: Positive
        if (isPosA && !isPosB) return -1;
        if (!isPosA && isPosB) return 1;
        if (isPosA && isPosB) {
          if (b.netProfit !== a.netProfit) return b.netProfit - a.netProfit;
          return a.symbol.localeCompare(b.symbol);
        }

        // Group 2: Negative vs Zero
        if (isNegA && isZeroB) return -1;
        if (isZeroA && isNegB) return 1;
        if (isNegA && isNegB) {
          if (b.netProfit !== a.netProfit) return b.netProfit - a.netProfit;
          return a.symbol.localeCompare(b.symbol);
        }

        // Group 3: Both zero
        return a.symbol.localeCompare(b.symbol);
      }

      return 0;
    });
  }, [tickersWithStats, searchQuery, sortOption]);

  const venueBadgeColor = (v: string) => {
    switch (v) {
      case "HL":
        return "bg-teal-500/10 text-teal-600 border-teal-500/20";
      case "LGT":
        return "bg-indigo-500/10 text-indigo-600 border-indigo-500/20";
      case "VNT":
        return "bg-emerald-500/10 text-emerald-600 border-emerald-500/20";
      case "RH":
        return "bg-amber-500/10 text-amber-600 border-amber-500/20";
      default:
        return "bg-slate-500/10 text-slate-600 border-slate-500/20";
    }
  };


  return (
    <aside className="w-80 border-r border-surface-border bg-surface flex flex-col justify-between select-none h-full overflow-hidden">
      {/* Top Header & Search */}
      <div className="p-3 border-b border-surface-border space-y-2.5">
        <div className="flex items-center justify-between">
          <div>
            <div className="text-[11px] font-mono font-bold uppercase tracking-wider text-slate-900 flex items-center space-x-1.5">
              <span>Perp Tickers</span>
              <span className="text-[10px] font-normal text-slate-600">({processedTickers.length}/{PERP_TICKERS.length})</span>
            </div>
            <div className="text-[10px] text-slate-600">Cross-DEX Union (≥2 Venues)</div>
          </div>
          <div className="flex items-center space-x-1">
            <span className="w-1.5 h-1.5 rounded-full bg-profit animate-pulse" />
            <span className="text-[10px] font-mono font-semibold text-profit">Live</span>
          </div>
        </div>

        {/* Search Bar */}
        <div className="relative">
          <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search 52 tickers (e.g. BTC, SOL)..."
            className="w-full pl-8 pr-7 py-1.5 bg-surface-raised border border-surface-border rounded-lg text-xs font-mono text-slate-900 placeholder-slate-600 focus:outline-none focus:ring-1 focus:ring-accent focus:border-accent transition"
          />
          {searchQuery && (
            <button
              onClick={() => setSearchQuery("")}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-slate-400 hover:text-slate-600 p-0.5"
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </div>

        {/* Sorting Controls */}
        <div className="grid grid-cols-3 gap-1 bg-surface-raised p-1 rounded-lg border border-surface-border text-[11px] font-mono">
          <button
            onClick={() => setSortOption("alpha")}
            className={`flex items-center justify-center space-x-1 py-1 px-1.5 rounded transition ${
              sortOption === "alpha"
                ? "bg-surface text-accent font-semibold shadow-xs border border-accent-line/60"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Sort alphabetically (A-Z)"
          >
            <ArrowDownAZ className="w-3 h-3" />
            <span>A-Z</span>
          </button>

          <button
            onClick={() => setSortOption("trades")}
            className={`flex items-center justify-center space-x-1 py-1 px-1.5 rounded transition ${
              sortOption === "trades"
                ? "bg-surface text-accent font-semibold shadow-xs border border-accent-line/60"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Sort by strategy transaction count (highest first)"
          >
            <Activity className="w-3 h-3" />
            <span>Trades</span>
          </button>

          <button
            onClick={() => setSortOption("pnl")}
            className={`flex items-center justify-center space-x-1 py-1 px-1.5 rounded transition ${
              sortOption === "pnl"
                ? "bg-surface text-accent font-semibold shadow-xs border border-accent-line/60"
                : "text-slate-600 hover:text-slate-900"
            }`}
            title="Sort by strategy net profit: positive first, then negative, then zero"
          >
            <DollarSign className="w-3 h-3" />
            <span>Net PnL</span>
          </button>
        </div>
      </div>

      {/* Scrollable Ticker List */}
      <div className="flex-1 overflow-y-auto px-2 py-2 space-y-1.5">
        {processedTickers.length === 0 ? (
          <div className="py-8 text-center text-xs font-mono text-slate-600">
            No tickers matching &quot;{searchQuery}&quot;
          </div>
        ) : (
          processedTickers.map((ticker) => {
            const isSelected = selectedSymbol === ticker.symbol;
            const hasTrades = ticker.tradeCount > 0;
            const isProfit = ticker.netProfit > 0;
            const isLoss = ticker.netProfit < 0;

            return (
              <button
                key={ticker.symbol}
                onClick={() => onSelectTicker(ticker.symbol)}
                className={`w-full p-2.5 rounded-lg border text-left transition relative group font-mono ${
                  isSelected
                    ? "bg-accent-dim/60 border-accent shadow-xs text-slate-900"
                    : "bg-surface hover:bg-surface-raised border-surface-border text-slate-700"
                }`}
              >
                {/* Active Indicator Strip */}
                {isSelected && (
                  <div className="absolute left-0 top-1.5 bottom-1.5 w-1 bg-accent rounded-r" />
                )}

                {/* Top Row: Symbol, Name & Venue Tags */}
                <div className="flex items-center justify-between mb-1.5 gap-1.5">
                  <div className="flex items-center space-x-1.5 min-w-0 whitespace-nowrap">
                    <span className="font-bold text-xs text-slate-900 tracking-tight shrink-0">
                      {ticker.base}
                    </span>
                    <span className="text-[10px] text-slate-500 font-mono shrink-0">-{ticker.quote}</span>
                    <span className="text-[9px] px-1 py-0.2 rounded bg-slate-200/60 text-slate-600 font-medium shrink-0">
                      PERP
                    </span>
                  </div>

                  {/* Venue Availability Badges (≥ 2 venues) */}
                  <div className="flex items-center space-x-1 shrink-0">
                    {ticker.venues.map((v) => (
                      <span
                        key={v}
                        className={`px-1 py-0.2 rounded text-[9px] font-semibold border ${venueBadgeColor(
                          v
                        )}`}
                        title={`Available on ${v}`}
                      >
                        {v}
                      </span>
                    ))}
                  </div>
                </div>

                {/* Bottom Row: Price, Strategy Trade Count, Net PnL */}
                <div className="flex items-center justify-between text-[11px] pt-1 border-t border-surface-border/50">
                  <span className="text-slate-600 text-[10px]">
                    {formatCurrencyPrice(ticker.basePrice)}
                  </span>

                  <div className="flex items-center space-x-2">
                    {/* Trade Count */}
                    <span
                      className={`text-[10px] px-1.5 py-0.2 rounded ${
                        hasTrades
                          ? "bg-surface-raised font-medium text-slate-700 border border-surface-border"
                          : "text-slate-500"
                      }`}
                      title={`${ticker.tradeCount} transactions executed by strategy`}
                    >
                      {ticker.tradeCount} tx{ticker.tradeCount === 1 ? "" : "s"}
                    </span>

                    {/* Net Profit */}
                    <span
                      className={`text-[10px] font-semibold px-1.5 py-0.2 rounded ${
                        isProfit
                          ? "text-profit bg-profit-dim border border-profit-border"
                          : isLoss
                          ? "text-loss bg-loss-dim border border-loss-border"
                          : "text-slate-500 bg-surface-raised border border-surface-border"
                      }`}
                      title={`Net profit generated: ${formatCurrencyPnl(ticker.netProfit)}`}
                    >
                      {formatCurrencyPnl(ticker.netProfit)}
                    </span>
                  </div>
                </div>

                {/* Optional Strategy Configuration Badge Row */}
                {(() => {
                  const cfg = strategyConfigs?.get(ticker.symbol);
                  if (!cfg) return null;
                  const tp = Number(cfg.targetProfitBps ?? cfg.target_profit_bps ?? 5.0);
                  const curUnits = Number(cfg.currentAccumulatedUnits ?? cfg.current_accumulated_units ?? 0);
                  const maxUnits = Number(cfg.maxAccumulatedUnits ?? cfg.max_accumulated_units ?? 3);
                  const mode = cfg.executionMode ?? cfg.execution_mode ?? "SINGLE_WAY";
                  return (
                    <div className="flex items-center space-x-1.5 pt-1 text-[9px] font-mono text-slate-500">
                      <span className="bg-blue-50 text-blue-700 px-1 py-0.2 rounded border border-blue-200/60 font-medium">
                        TP {isNaN(tp) ? "5.0" : tp.toFixed(1)} bps
                      </span>
                      <span className="bg-slate-100 text-slate-700 px-1 py-0.2 rounded border border-slate-200">
                        {curUnits}/{maxUnits}u
                      </span>
                      <span className="text-slate-400">
                        {mode === "DOUBLE_WAY" ? "DW" : "SW"}
                      </span>
                    </div>
                  );
                })()}
              </button>
            );
          })
        )}
      </div>

      {/* Bottom Panel Status & Venues */}
      <div className="p-3 border-t border-surface-border bg-surface-raised text-[10px] font-mono text-slate-600 space-y-1">
        <div className="flex justify-between items-center">
          <span className="text-slate-600">Tauri Core:</span>
          <span className="text-accent font-semibold flex items-center space-x-1">
            <span className="w-1.5 h-1.5 rounded-full bg-accent" />
            <span>IPC Connected</span>
          </span>
        </div>
        <div className="flex justify-between items-center">
          <span className="text-slate-600">Perp DEXes:</span>
          <span className="text-slate-700 font-medium">HL · LGT · VNT · RH</span>
        </div>
        <div className="flex justify-between items-center">
          <span className="text-slate-600">Union Coverage:</span>
          <span className="text-profit font-medium">52 Pairs (≥2 Venues)</span>
        </div>
      </div>
    </aside>
  );
};
