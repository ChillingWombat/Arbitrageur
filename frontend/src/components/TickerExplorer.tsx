import React, { useState, useMemo, useRef, useEffect } from "react";
import {
  Search,
  X,
  ArrowDownAZ,
  Activity,
  DollarSign,
  Settings,
  SlidersHorizontal,
  ArrowUpDown,
  Check,
  AlertTriangle,
} from "lucide-react";
import {
  PERP_TICKERS,
  formatCurrencyPnl,
  getTickerActiveVenues,
  type PerpTicker,
  type TickerSortOption,
  type TickerWithStats,
} from "../types/tickers";
import { SwitchToggle } from "./SwitchToggle";
import type { ExecutionLogEntry, TickerStrategyConfigDto, SystemStatusDto, RiskOverviewDto } from "../types";

interface TickerExplorerProps {
  selectedSymbol: string;
  onSelectTicker: (symbol: string) => void;
  executionLogs: ExecutionLogEntry[];
  strategyConfigs?: Map<string, TickerStrategyConfigDto>;
  status?: SystemStatusDto | null;
  riskData?: RiskOverviewDto | null;
  enabledTickers?: Set<string>;
  onToggleTicker?: (symbol: string) => void;
  onOpenSettings?: () => void;
  onOpenTickerConfig?: (symbol: string) => void;
  availableTickers?: PerpTicker[];
  activeVenueCodes?: Set<string>;
}

export const TickerExplorer: React.FC<TickerExplorerProps> = ({
  selectedSymbol,
  onSelectTicker,
  executionLogs,
  strategyConfigs: _strategyConfigs,
  status,
  riskData,
  enabledTickers,
  onToggleTicker,
  onOpenSettings,
  onOpenTickerConfig,
  availableTickers,
  activeVenueCodes,
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

  // Compute stats (transaction count, net profit, net bps, and risk/balance issues) per ticker
  const tickersWithStats: TickerWithStats[] = useMemo(() => {
    const statsMap = new Map<string, { count: number; pnl: number; pnlBps: number; lastTime?: string }>();

    for (const log of executionLogs) {
      if (!log.symbol) continue;
      let baseKey = log.symbol.toUpperCase().split(/[-_/]/)[0];
      if (baseKey.startsWith("1000") && baseKey.length > 4) {
        baseKey = baseKey.slice(4);
      } else if (baseKey.startsWith("K") && baseKey.length > 3) {
        baseKey = baseKey.slice(1);
      }

      const cur = statsMap.get(baseKey) || { count: 0, pnl: 0, pnlBps: 0 };
      cur.count += 1;
      cur.pnl += log.net_profit || 0;
      if (log.trigger_spread_bps) {
        cur.pnlBps += log.trigger_spread_bps;
      }
      if (!cur.lastTime || log.timestamp > cur.lastTime) {
        cur.lastTime = log.timestamp;
      }
      statsMap.set(baseKey, cur);
    }

    const baseList = availableTickers ?? PERP_TICKERS;
    return baseList.map((t) => {
      const stat = statsMap.get(t.base.toUpperCase()) || { count: 0, pnl: 0, pnlBps: 0 };
      const netProfit = Math.round(stat.pnl * 100) / 100;
      const netProfitBps =
        stat.count > 0 && stat.pnlBps
          ? Math.round((stat.pnlBps / stat.count) * 10) / 10
          : t.basePrice > 0 && Math.abs(netProfit) > 0
          ? Math.round((netProfit / (t.basePrice * 0.05)) * 10000 * 10) / 10
          : 0;

      // Detect if positions are not balanced or there are other issues
      const netDelta = riskData?.net_deltas?.find((d) => d.asset.toUpperCase() === t.base.toUpperCase());
      const hasUnbalancedPosition = Boolean(
        netDelta && (!netDelta.is_neutral || Math.abs(netDelta.net_quantity) > 0.0001)
      );
      const config = _strategyConfigs?.get(t.symbol);
      const hasConfigIssue = Boolean(config && config.currentAccumulatedUnits > config.maxAccumulatedUnits);
      const hasIssue = hasUnbalancedPosition || hasConfigIssue;
      const issueDescription = hasUnbalancedPosition
        ? `Unbalanced Position (Net Δ: ${netDelta?.net_quantity.toFixed(3)} ${t.base})`
        : hasConfigIssue
        ? "Exceeded Max Grid Units"
        : undefined;

      return {
        ...t,
        tradeCount: stat.count,
        netProfit,
        netProfitBps,
        hasIssue,
        issueDescription,
        lastTradeTime: stat.lastTime,
      };
    });
  }, [availableTickers, executionLogs, riskData, _strategyConfigs]);

  // Filter and sort the tickers
  const processedTickers = useMemo(() => {
    // 1. Filter by query
    let list = tickersWithStats;
    const query = searchQuery.trim().toLowerCase();
    if (query) {
      list = list.filter((t) => {
        const activeVenues = activeVenueCodes ? getTickerActiveVenues(t, activeVenueCodes) : t.venues;
        return (
          t.symbol.toLowerCase().includes(query) ||
          t.base.toLowerCase().includes(query) ||
          t.name.toLowerCase().includes(query) ||
          t.quote.toLowerCase().includes(query) ||
          query === "perp" ||
          activeVenues.some((v) => v.toLowerCase().includes(query))
        );
      });
    }

    // 2. Sort: Issues ALWAYS prioritized at the top of the list!
    return [...list].sort((a, b) => {
      if (a.hasIssue && !b.hasIssue) return -1;
      if (!a.hasIssue && b.hasIssue) return 1;

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
  }, [tickersWithStats, searchQuery, sortOption, activeVenueCodes]);

  const totalAvailableCount = (availableTickers ?? PERP_TICKERS).length;

  return (
    <aside className="w-80 border-r border-surface-border bg-surface flex flex-col justify-between select-none h-full overflow-hidden">
      {/* Top Header & Search */}
      <div className="p-3 border-b border-surface-border space-y-2.5">
        <div className="flex items-center justify-between">
          <div className="text-[11px] font-mono font-bold uppercase tracking-wider text-slate-900 flex items-center space-x-1.5">
            <span>Tickers</span>
            <span className="text-[10px] font-normal text-slate-600">({processedTickers.length}/{totalAvailableCount})</span>
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
              placeholder={`Search ${totalAvailableCount} tickers...`}
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

      {/* Column Subheader */}
      <div className="px-3 py-1.5 border-b border-surface-border bg-surface-raised/60 flex items-center justify-between text-[10px] font-mono text-slate-600 font-semibold uppercase tracking-wider">
        <span>Symbol</span>
        <span className="text-right">Net PnL (USD / BPS)</span>
      </div>

      {/* Scrollable Ticker List */}
      <div className="flex-1 overflow-y-auto px-2 py-1.5 space-y-1">
        {processedTickers.length === 0 ? (
          <div className="py-10 text-center text-xs font-mono text-slate-600 px-4 space-y-2.5">
            {searchQuery ? (
              <div>No tickers matching &quot;{searchQuery}&quot;</div>
            ) : (
              <>
                <div className="text-slate-800 font-bold text-sm">No Available Tickers</div>
                <div className="text-[11px] text-slate-500 leading-relaxed">
                  Cross-market arbitrage requires at least 2 enabled &amp; connected exchanges in Settings.
                </div>
                {onOpenSettings && (
                  <button
                    type="button"
                    onClick={onOpenSettings}
                    className="mt-2 inline-flex items-center space-x-1.5 px-3 py-1.5 text-xs bg-accent text-white rounded-lg font-medium hover:bg-accent-hover transition cursor-pointer shadow-xs"
                  >
                    <Settings className="w-3.5 h-3.5" />
                    <span>Open Exchange Settings</span>
                  </button>
                )}
              </>
            )}
          </div>
        ) : (
          processedTickers.map((ticker) => {
            const isSelected = selectedSymbol === ticker.symbol;
            const hasTrades = ticker.tradeCount > 0;
            const isProfit = ticker.netProfit > 0;
            const isLoss = ticker.netProfit < 0;
            const isEnabled = enabledTickers ? enabledTickers.has(ticker.symbol) : true;
            const hasIssue = ticker.hasIssue;

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
                className={`w-full px-2.5 py-2 rounded-lg border text-left transition relative group font-mono cursor-pointer flex items-center justify-between ${
                  hasIssue
                    ? "bg-red-50/90 border-red-300 text-red-900 shadow-2xs hover:bg-red-100/90"
                    : !isEnabled
                    ? "opacity-40 grayscale bg-slate-100/60 border-slate-200 hover:opacity-60"
                    : isSelected
                    ? "bg-accent-dim/70 border-accent/60 shadow-2xs text-slate-900 border-l-[3px] border-l-accent"
                    : "bg-surface hover:bg-surface-raised border-surface-border text-slate-700 hover:border-slate-300"
                }`}
              >
                {/* Left: Symbol & Status */}
                <div className="flex items-center space-x-2 min-w-0 pr-1">
                  {hasIssue ? (
                    <span title={ticker.issueDescription || "Position Unbalanced or Risk Issue"} className="shrink-0 flex items-center">
                      <AlertTriangle className="w-3.5 h-3.5 text-loss animate-pulse" />
                    </span>
                  ) : null}
                  <div className="flex items-center space-x-1">
                    <span className={`font-bold text-xs tracking-tight ${hasIssue ? "text-red-950 font-black" : "text-slate-900"}`}>
                      {ticker.base}
                    </span>
                    <span className="text-[8px] px-1 py-0.2 rounded bg-slate-200/70 text-slate-600 font-semibold">
                      PERP
                    </span>
                  </div>
                  {hasIssue && (
                    <span className="text-[8px] font-bold px-1.5 py-0.2 rounded bg-red-100 text-red-700 border border-red-200 uppercase">
                      ISSUE
                    </span>
                  )}
                  {!isEnabled && (
                    <span className="text-[8px] font-medium px-1 py-0.2 rounded bg-slate-200/60 text-slate-500 uppercase">
                      OFF
                    </span>
                  )}
                </div>

                {/* Right: Net PnL (USD + BPS) & Actions */}
                <div className="flex items-center space-x-2 shrink-0">
                  <div className="flex flex-col items-end">
                    <span
                      className={`text-[11px] font-bold tabular-nums ${
                        isProfit ? "text-profit" : isLoss ? "text-loss" : "text-slate-700"
                      }`}
                      title={hasTrades ? `${ticker.tradeCount} txs · ${formatCurrencyPnl(ticker.netProfit)}` : "No trades executed"}
                    >
                      {formatCurrencyPnl(ticker.netProfit)}
                    </span>
                    <span
                      className={`text-[9px] font-mono tabular-nums leading-none ${
                        isProfit ? "text-profit" : isLoss ? "text-loss" : "text-slate-400"
                      }`}
                    >
                      {ticker.netProfitBps && ticker.netProfitBps > 0
                        ? `+${ticker.netProfitBps.toFixed(1)} bps`
                        : ticker.netProfitBps && ticker.netProfitBps < 0
                        ? `${ticker.netProfitBps.toFixed(1)} bps`
                        : "0.0 bps"}
                    </span>
                  </div>

                  {/* On/Off Switch Toggle */}
                  {onToggleTicker && (
                    <div
                      onClick={(e) => e.stopPropagation()}
                      title={isEnabled ? `Switch off ${ticker.symbol}` : `Switch on ${ticker.symbol}`}
                      className="shrink-0 flex items-center"
                    >
                      <SwitchToggle
                        checked={isEnabled}
                        onChange={() => onToggleTicker(ticker.symbol)}
                        size="sm"
                      />
                    </div>
                  )}

                  {/* Configure strategy button */}
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenTickerConfig?.(ticker.symbol);
                    }}
                    className="p-1 rounded text-slate-600 hover:text-accent hover:bg-surface border border-transparent hover:border-surface-border transition cursor-pointer opacity-70 group-hover:opacity-100"
                    title={`Configure ${ticker.symbol} parameters`}
                  >
                    <SlidersHorizontal className="w-3 h-3" />
                  </button>
                </div>
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
            className={`px-2.5 py-1 rounded-lg border text-xs font-mono font-medium ${
              isLive
                ? "bg-amber-50 border-amber-300 text-amber-800"
                : "bg-blue-50 border-blue-200 text-blue-700"
            }`}
            title={isLive ? "Real Trading (Live Execution Active)" : "Paper Trading (Live Feeds & Simulated Orderbooks)"}
          >
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
