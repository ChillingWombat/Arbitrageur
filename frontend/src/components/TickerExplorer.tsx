import React, { useState, useMemo, useRef, useEffect } from "react";
import { Search, X, ArrowDownAZ, Activity, DollarSign, Settings, SlidersHorizontal, ArrowUpDown, Check } from "lucide-react";
import {
  PERP_TICKERS,
  formatCurrencyPrice,
  formatCurrencyPnl,
  type TickerSortOption,
  type TickerWithStats,
} from "../types/tickers";
import type { ExecutionLogEntry, TickerStrategyConfigDto, SystemStatusDto } from "../types";

interface TickerExplorerProps {
  selectedSymbol: string;
  onSelectTicker: (symbol: string) => void;
  executionLogs: ExecutionLogEntry[];
  strategyConfigs?: Map<string, TickerStrategyConfigDto>;
  status?: SystemStatusDto | null;
  onOpenSettings?: () => void;
  onOpenTickerConfig?: (symbol: string) => void;
}

export const TickerExplorer: React.FC<TickerExplorerProps> = ({
  selectedSymbol,
  onSelectTicker,
  executionLogs,
  strategyConfigs,
  status,
  onOpenSettings,
  onOpenTickerConfig,
}) => {
  const pnl = status?.total_realized_pnl ?? 0;
  const isLive = status?.is_live_mode ?? false;
  const [searchQuery, setSearchQuery] = useState("");
  const [sortOption, setSortOption] = useState<TickerSortOption>("alpha");
  const [showSortMenu, setShowSortMenu] = useState(false);
  const sortMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (sortMenuRef.current && !sortMenuRef.current.contains(event.target as Node)) {
        setShowSortMenu(false);
      }
    };
    if (showSortMenu) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, [showSortMenu]);

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
          <div className="text-[11px] font-mono font-bold uppercase tracking-wider text-slate-900 flex items-center space-x-1.5">
            <span>Tickers</span>
            <span className="text-[10px] font-normal text-slate-600">({processedTickers.length}/{PERP_TICKERS.length})</span>
          </div>
        </div>

        {/* Search Bar & Sort Configuration Button */}
        <div className="flex items-center space-x-1.5 relative">
          <div className="relative flex-1">
            <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-slate-400 pointer-events-none" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search 52 tickers..."
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

          {/* Tiny Sort Configuration Button & Popover */}
          <div className="relative" ref={sortMenuRef}>
            <button
              onClick={() => setShowSortMenu((prev) => !prev)}
              aria-label="Sort configuration"
              title={`Sort by: ${
                sortOption === "alpha"
                  ? "Alphabetical (A-Z)"
                  : sortOption === "trades"
                  ? "Strategy Trades"
                  : "Net Profit / Loss"
              }`}
              className={`p-1.5 rounded-lg border transition flex items-center justify-center relative ${
                showSortMenu
                  ? "bg-accent/10 border-accent text-accent"
                  : sortOption !== "alpha"
                  ? "bg-accent/5 border-accent-line text-accent"
                  : "bg-surface-raised border-surface-border text-slate-600 hover:text-slate-900 hover:border-slate-300"
              }`}
            >
              <ArrowUpDown className="w-3.5 h-3.5" />
              {sortOption !== "alpha" && (
                <span className="absolute -top-0.5 -right-0.5 w-1.5 h-1.5 rounded-full bg-accent" />
              )}
            </button>

            {/* Floating Popover Menu */}
            {showSortMenu && (
              <div className="absolute right-0 top-full mt-1.5 w-48 bg-surface border border-surface-border rounded-lg shadow-lg py-1 z-30 font-mono text-xs animate-in fade-in zoom-in-95 duration-100">
                <div className="px-2.5 py-1 text-[10px] uppercase font-bold text-slate-600 border-b border-surface-border/60">
                  Sort Tickers
                </div>

                <button
                  onClick={() => {
                    setSortOption("alpha");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition ${
                    sortOption === "alpha"
                      ? "bg-accent/10 text-accent font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <ArrowDownAZ className="w-3.5 h-3.5 text-slate-600" />
                    <span>A-Z (Name)</span>
                  </div>
                  {sortOption === "alpha" && <Check className="w-3.5 h-3.5 text-accent" />}
                </button>

                <button
                  onClick={() => {
                    setSortOption("trades");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition ${
                    sortOption === "trades"
                      ? "bg-accent/10 text-accent font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <Activity className="w-3.5 h-3.5 text-slate-600" />
                    <span>Strategy Trades</span>
                  </div>
                  {sortOption === "trades" && <Check className="w-3.5 h-3.5 text-accent" />}
                </button>

                <button
                  onClick={() => {
                    setSortOption("pnl");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition ${
                    sortOption === "pnl"
                      ? "bg-accent/10 text-accent font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <DollarSign className="w-3.5 h-3.5 text-slate-600" />
                    <span>Net Profit / Loss</span>
                  </div>
                  {sortOption === "pnl" && <Check className="w-3.5 h-3.5 text-accent" />}
                </button>
              </div>
            )}
          </div>
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
              <div
                key={ticker.symbol}
                role="button"
                tabIndex={0}
                onClick={() => onSelectTicker(ticker.symbol)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onSelectTicker(ticker.symbol);
                  }
                }}
                className={`w-full p-2.5 rounded-lg border text-left transition relative group font-mono cursor-pointer ${
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

                {/* Strategy Configuration Badge Row & Configure Button */}
                {(() => {
                  const cfg = strategyConfigs?.get(ticker.symbol);
                  const tp = Number(cfg?.targetProfitBps ?? cfg?.target_profit_bps ?? 5.0);
                  const curUnits = Number(cfg?.currentAccumulatedUnits ?? cfg?.current_accumulated_units ?? 0);
                  const maxUnits = Number(cfg?.maxAccumulatedUnits ?? cfg?.max_accumulated_units ?? 3);
                  const mode = cfg?.executionMode ?? cfg?.execution_mode ?? "SINGLE_WAY";
                  return (
                    <div className="flex items-center justify-between pt-1 text-[9px] font-mono text-slate-500">
                      <div className="flex items-center space-x-1.5">
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
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          onOpenTickerConfig?.(ticker.symbol);
                        }}
                        className="p-1 rounded text-slate-400 hover:text-accent hover:bg-surface-raised border border-transparent hover:border-surface-border transition cursor-pointer"
                        title={`Configure ${ticker.symbol} quantitative parameters`}
                      >
                        <SlidersHorizontal className="w-3 h-3" />
                      </button>
                    </div>
                  );
                })()}
              </div>
            );
          })
        )}
      </div>

      {/* Bottom Control Panel: Net Profit, Paper/Live Indicator & Settings */}
      <div className="p-3 border-t border-surface-border bg-surface-raised space-y-2.5">
        <div className="flex items-center justify-between">
          <div className="flex flex-col">
            <span className="text-[9px] uppercase font-mono tracking-wider text-slate-500 font-semibold">
              Net Profit
            </span>
            <span
              className={`font-mono text-xs font-bold ${
                pnl >= 0 ? "text-profit" : "text-loss"
              }`}
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          <div
            className={`px-2.5 py-1 rounded-lg border flex items-center space-x-1.5 text-xs font-mono font-medium ${
              isLive
                ? "bg-amber-50 border-amber-300 text-amber-800"
                : "bg-blue-50 border-blue-200 text-blue-700"
            }`}
            title={isLive ? "Real Trading (Live Execution Active)" : "Paper Trading (Live Feeds & Simulated Orderbooks)"}
          >
            <span className={`w-1.5 h-1.5 rounded-full ${isLive ? "bg-amber-600 animate-pulse" : "bg-blue-600"}`} />
            <span>{isLive ? "Live" : "Paper"}</span>
          </div>
        </div>

        <button
          type="button"
          onClick={onOpenSettings}
          className="w-full py-1.5 px-3 bg-surface hover:bg-slate-100 text-slate-700 hover:text-slate-900 border border-surface-border rounded-lg text-xs font-mono font-medium transition shadow-xs flex items-center justify-center space-x-2 cursor-pointer"
          title="Engine Settings & API Keys"
        >
          <Settings className="w-3.5 h-3.5 text-slate-500" />
          <span>Settings</span>
        </button>
      </div>
    </aside>
  );
};
