import React, { useState, useMemo, useRef, useEffect, useSyncExternalStore, useCallback } from "react";
import {
  Search,
  X,
  ArrowDownAZ,
  Activity,
  DollarSign,
  Settings,
  ArrowUpDown,
  Check,
  AlertTriangle,
  Droplets,
  Filter,
  EyeOff,
} from "lucide-react";
import {
  PERP_TICKERS,
  formatCurrencyPnl,
  getTickerActiveVenues,
  getTickerAvgVolumeUsd,
  tickerUnavailabilityReason,
  type PerpTicker,
  type TickerSortOption,
  type TickerWithStats,
} from "../types/tickers";
import { EmptyState } from "./EmptyState";
import { api } from "../services/api";
import {
  getBotPhaseRegistryVersion,
  subscribeBotPhaseRegistry,
  tickerBotStatus,
  type TickerBotStatus,
} from "../hooks/useBotLifecycle";
import type { ExecutionLogEntry, TickerStrategyConfigDto, RiskOverviewDto, SystemStatusDto, VenueVolumesDto, StrategyParamsDto } from "../types";
import { deriveStepSizing } from "../services/contractSpecs";

interface TickerExplorerProps {
  selectedSymbol: string | null;
  onSelectTicker: (symbol: string) => void;
  executionLogs: ExecutionLogEntry[];
  strategyConfigs?: Map<string, TickerStrategyConfigDto>;
  /** Universal Simple Spread sizing params + venue equity — the per-ticker
   *  grid cap derives from these, not from an authored max. */
  futuresParams?: StrategyParamsDto | null;
  totalEquityUsd?: number | null;
  riskData?: RiskOverviewDto | null;
  enabledTickers?: Set<string>;
  onToggleTicker?: (symbol: string) => void;
  onOpenSettings?: () => void;
  onOpenTickerConfig?: (symbol: string) => void;
  availableTickers?: PerpTicker[];
  activeVenueCodes?: Set<string>;
  tickerVolumes?: VenueVolumesDto[];
  /** Backend status — drives the empty-state's honest unavailability reason. */
  systemStatus?: SystemStatusDto | null;
  /** Pre-computed empty-universe explanation from App, which can see the
   *  live market-universe response and the pre-exemption intersection —
   *  the component alone cannot tell "no overlap" from "all hidden". */
  tickerUnavailableReason?: string;
  onHideTicker?: (base: string) => void;
}

const formatCompactUsd = (v: number): string => {
  if (v >= 1e9) return `$${(v / 1e9).toFixed(1)}B`;
  if (v >= 1e6) return `$${(v / 1e6).toFixed(1)}M`;
  if (v >= 1e3) return `$${(v / 1e3).toFixed(1)}K`;
  return `$${v.toFixed(0)}`;
};

interface TickerRowProps {
  symbol: string;
  base: string;
  quote: string;
  isSelected: boolean;
  isEnabled: boolean;
  /** Live bot lifecycle for this ticker — "running" gets a tag by the name. */
  botStatus: TickerBotStatus;
  hasIssue: boolean | undefined;
  issueDescription: string | undefined;
  tradeCount: number;
  netProfit: number;
  netProfitBps: number | undefined;
  avgEntrySpreadBps?: number;
  avgVolumeUsd: number | undefined;
  showLiquidity: boolean;
  onSelect: (symbol: string) => void;
  onContextMenu: (e: React.MouseEvent, symbol: string, base: string) => void;
}

const TickerRow: React.FC<TickerRowProps> = React.memo(
  ({
    symbol,
    base,
    quote,
    isSelected,
    isEnabled,
    botStatus,
    hasIssue,
    issueDescription,
    tradeCount,
    netProfit,
    netProfitBps,
    avgEntrySpreadBps,
    avgVolumeUsd,
    showLiquidity,
    onSelect,
    onContextMenu,
  }) => {
    const hasTrades = tradeCount > 0;
    const isProfit = netProfit > 0;
    const isLoss = netProfit < 0;
    const handleContextMenu = useCallback(
      (e: React.MouseEvent) => {
        e.preventDefault();
        e.stopPropagation();
        onContextMenu(e, symbol, base);
      },
      [onContextMenu, symbol, base],
    );
    const handleKeyDown = useCallback(
      (e: React.KeyboardEvent) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect(symbol);
        }
      },
      [onSelect, symbol],
    );
    const handleSelect = useCallback(() => onSelect(symbol), [onSelect, symbol]);
    return (
      <div
        role="button"
        tabIndex={0}
        title="Right-click to hide"
        onClick={handleSelect}
        onContextMenu={handleContextMenu}
        onKeyDown={handleKeyDown}
        className={`w-full px-2.5 py-2 rounded-lg border text-left transition relative group cursor-pointer flex items-center justify-between ${
          hasIssue
            ? "bg-loss-dim border-loss-border text-loss shadow-2xs hover:bg-red-100/70"
            : isSelected
            ? "bg-blue-50 border-blue-300 shadow-2xs text-slate-900"
            : !isEnabled
            ? "opacity-40 grayscale bg-slate-100/60 border-slate-200 hover:opacity-60"
            : "bg-surface hover:bg-surface-raised border-surface-border text-slate-700 hover:border-slate-300 shadow-2xs"
        }`}
      >
        {/* Left: Symbol & Status */}
        <div className="flex items-center space-x-2 min-w-0 pr-1">
          <div className="flex items-center space-x-1.5">
            <span className={`font-bold text-xs font-sans tracking-tight ${hasIssue ? "text-loss font-black" : "text-slate-900"}`}>
              {base}
              {/* Distinct stablecoin quotes are distinct instruments (XRP-USDC
                  vs XRP-USDT trade different venue sets) — the quote suffix is
                  what keeps their rows visually distinguishable. */}
              <span className="text-muted font-semibold">-{quote}</span>
            </span>
            <span className="text-[9px] font-mono px-1.5 py-px rounded bg-slate-100 text-slate-600 font-bold border border-slate-200/60">
              Perp
            </span>
            {botStatus === "running" && (
              <span
                className="inline-flex items-center gap-1 text-[9px] font-sans px-1.5 py-px rounded bg-profit-dim text-profit font-bold border border-profit-border"
                title="Bot is running for this ticker — new entries allowed"
              >
                <span className="w-1 h-1 rounded-full bg-profit animate-pulse" />
                Running
              </span>
            )}
          </div>
          {hasIssue ? (
            <span title={issueDescription || "Position Unbalanced or Risk Issue"} className="shrink-0 flex items-center">
              <AlertTriangle className="w-3.5 h-3.5 text-loss animate-pulse" />
            </span>
          ) : null}
        </div>

        {/* Right: Net PnL (USD + BPS) & Actions */}
        <div className="flex items-center space-x-2 shrink-0">
          <div className="flex flex-col items-end">
            <span
              className={`text-xs font-bold tabular-nums ${
                isProfit ? "text-profit" : isLoss ? "text-loss" : "text-slate-700"
              }`}
              title={
                hasTrades
                  ? `${tradeCount} txs · ${formatCurrencyPnl(netProfit)}${
                      avgEntrySpreadBps != null
                        ? ` (avg entry sprd: ${avgEntrySpreadBps >= 0 ? "+" : ""}${avgEntrySpreadBps.toFixed(1)} bps)`
                        : ""
                    }`
                  : "No trades executed"
              }
            >
              {formatCurrencyPnl(netProfit)}
            </span>
            <span
              className={`text-[11px] font-mono tabular-nums leading-none ${
                showLiquidity ? "text-slate-500" : "text-muted"
              }`}
              title={
                !showLiquidity && avgEntrySpreadBps != null
                  ? `entry sprd: ${avgEntrySpreadBps >= 0 ? "+" : ""}${avgEntrySpreadBps.toFixed(1)} bps`
                  : undefined
              }
            >
              {showLiquidity
                ? avgVolumeUsd !== undefined
                  ? `${formatCompactUsd(avgVolumeUsd)} 24h`
                  : "vol —"
                : netProfitBps && netProfitBps > 0
                ? `+${netProfitBps.toFixed(1)} bps`
                : netProfitBps && netProfitBps < 0
                ? `${netProfitBps.toFixed(1)} bps`
                : "0.0 bps"}
            </span>
          </div>
        </div>
      </div>
    );
  },
);

const TickerExplorerInner: React.FC<TickerExplorerProps> = ({
  selectedSymbol,
  onSelectTicker,
  executionLogs,
  strategyConfigs: _strategyConfigs,
  futuresParams = null,
  totalEquityUsd = null,
  riskData,
  enabledTickers,
  onToggleTicker: _onToggleTicker,
  onOpenSettings,
  onOpenTickerConfig: _onOpenTickerConfig,
  availableTickers,
  activeVenueCodes,
  tickerVolumes,
  systemStatus,
  tickerUnavailableReason,
  onHideTicker,
}) => {
  const [searchQuery, setSearchQuery] = useState("");
  const [sortOption, setSortOption] = useState<TickerSortOption>("alpha");
  const [showSortMenu, setShowSortMenu] = useState(false);
  const sortMenuRef = useRef<HTMLDivElement>(null);
  const [botFilter, setBotFilter] = useState<TickerBotStatus | "all">("all");
  const [showBotMenu, setShowBotMenu] = useState(false);
  const botMenuRef = useRef<HTMLDivElement>(null);
  const [contextMenu, setContextMenu] = useState<{ symbol: string; base: string; x: number; y: number } | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);
  // Live bot phases — re-render whenever any mounted bot publishes.
  const botPhaseVersion = useSyncExternalStore(subscribeBotPhaseRegistry, getBotPhaseRegistryVersion, getBotPhaseRegistryVersion);

  const handleTickerContextMenu = useCallback((e: React.MouseEvent, symbol: string, base: string) => {
    const MENU_W = 208; // w-52
    const MENU_H = 48; // single-row estimate; clamped below
    const MARGIN = 8;
    const x = Math.min(Math.max(e.clientX, MARGIN), Math.max(MARGIN, window.innerWidth - MENU_W - MARGIN));
    const y = Math.min(Math.max(e.clientY, MARGIN), Math.max(MARGIN, window.innerHeight - MENU_H - MARGIN));
    setContextMenu({ symbol, base, x, y });
  }, []);

  const handleSelectContextItem = useCallback(
    (id: string) => {
      const target = contextMenu;
      setContextMenu(null);
      if (!target) return;
      switch (id) {
        case "hide": {
          const b = target.base.toUpperCase();
          void api.setTickerExempted(b, true).catch(() => {});
          onHideTicker?.(b);
          break;
        }
        default:
          break;
      }
    },
    [contextMenu, onHideTicker],
  );

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (sortMenuRef.current && !sortMenuRef.current.contains(event.target as Node)) {
        setShowSortMenu(false);
      }
      if (botMenuRef.current && !botMenuRef.current.contains(event.target as Node)) {
        setShowBotMenu(false);
      }
      if (contextMenuRef.current && !contextMenuRef.current.contains(event.target as Node)) {
        setContextMenu(null);
      }
    };
    if (showSortMenu || showBotMenu || contextMenu) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, [showSortMenu, showBotMenu, contextMenu]);

  useEffect(() => {
    if (!contextMenu) return;
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setContextMenu(null);
    };
    const handleScroll = () => setContextMenu(null);
    document.addEventListener("keydown", handleKey);
    window.addEventListener("scroll", handleScroll, true);
    return () => {
      document.removeEventListener("keydown", handleKey);
      window.removeEventListener("scroll", handleScroll, true);
    };
  }, [contextMenu]);

  // Compute stats (transaction count, net profit, net bps, and risk/balance issues) per ticker
  const tickersWithStats: TickerWithStats[] = useMemo(() => {
    const statsMap = new Map<
      string,
      { count: number; pnl: number; entrySpreadBpsSum: number; entrySpreadCount: number; lastTime?: string }
    >();

    for (const log of executionLogs) {
      if (!log.symbol) continue;
      let baseKey = log.symbol.toUpperCase().split(/[-_/]/)[0];
      if (baseKey.startsWith("1000") && baseKey.length > 4) {
        baseKey = baseKey.slice(4);
      } else if (baseKey.startsWith("K") && baseKey.length > 3) {
        baseKey = baseKey.slice(1);
      }

      const cur = statsMap.get(baseKey) || {
        count: 0,
        pnl: 0,
        entrySpreadBpsSum: 0,
        entrySpreadCount: 0,
      };
      cur.count += 1;
      cur.pnl += log.net_profit || 0;
      if (log.trigger_spread_bps != null && Number.isFinite(log.trigger_spread_bps)) {
        cur.entrySpreadBpsSum += log.trigger_spread_bps;
        cur.entrySpreadCount += 1;
      }
      if (!cur.lastTime || log.timestamp > cur.lastTime) {
        cur.lastTime = log.timestamp;
      }
      statsMap.set(baseKey, cur);
    }

    // Dedupe by canonical symbol — upstream sources are already unique
    // (applyMarketUniverse keys a Map on BASE-QUOTE), but a static-catalog or
    // future-source duplicate must never render two rows under one React key.
    const seenSymbols = new Set<string>();
    const baseList = (availableTickers ?? PERP_TICKERS).filter((t) => {
      const key = t.symbol.toUpperCase();
      if (seenSymbols.has(key)) return false;
      seenSymbols.add(key);
      return true;
    });
    return baseList.map((t) => {
      const stat = statsMap.get(t.base.toUpperCase()) || {
        count: 0,
        pnl: 0,
        entrySpreadBpsSum: 0,
        entrySpreadCount: 0,
      };
      const netProfit = Math.round(stat.pnl * 100) / 100;

      // Detect if positions are not balanced or there are other issues
      const netDelta = riskData?.net_deltas?.find((d) => d.asset.toUpperCase() === t.base.toUpperCase());
      const hasUnbalancedPosition = Boolean(
        netDelta &&
          (!netDelta.is_neutral ||
            (Math.abs(netDelta.net_exposure_usd) >= 1.0 && Math.abs(netDelta.net_quantity) > 0.0001))
      );
      const config = _strategyConfigs?.get(t.symbol);
      const stepSizing = deriveStepSizing(t.symbol, t.venues, t.basePrice, totalEquityUsd, {
        stepSizeMult: futuresParams?.stepSizeMult ?? 10,
        maxStepUsd: futuresParams?.maxStepUsd ?? 2500,
        maxTickerEquityPct: futuresParams?.maxTickerEquityPct ?? 20,
      });
      const maxUnits = stepSizing.maxSteps;

      // Determine trade notional: derive from sizing, futuresParams maxStepUsd, or basePrice
      const stepNotionalUsd =
        stepSizing.stepBaseQty > 0 && t.basePrice > 0
          ? stepSizing.stepBaseQty * t.basePrice
          : (futuresParams?.maxStepUsd ?? 50);
      const tradeNotional = stepNotionalUsd > 0 ? stepNotionalUsd : (futuresParams?.maxStepUsd ?? 50);
      const totalNotional = Math.max(tradeNotional * Math.max(1, stat.count), 1);

      let netProfitBps = 0;
      if (netProfit !== 0 && totalNotional > 0) {
        const rawBps = (netProfit / totalNotional) * 10_000;
        netProfitBps = Math.round(rawBps * 10) / 10;
        // Crucially: ensure Math.sign(netProfitBps) === Math.sign(netProfit).
        // If netProfit < 0, netProfitBps is strictly negative!
        if (netProfit < 0 && netProfitBps >= 0) {
          netProfitBps = -0.1;
        } else if (netProfit > 0 && netProfitBps <= 0) {
          netProfitBps = 0.1;
        }
      }

      const avgEntrySpreadBps =
        stat.entrySpreadCount > 0
          ? Math.round((stat.entrySpreadBpsSum / stat.entrySpreadCount) * 10) / 10
          : undefined;

      const hasConfigIssue = Boolean(config && config.currentAccumulatedUnits > maxUnits);
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
        avgEntrySpreadBps,
        hasIssue,
        issueDescription,
        lastTradeTime: stat.lastTime,
        avgVolumeUsd: getTickerAvgVolumeUsd(t.base, tickerVolumes ?? []),
      };
    });
  }, [availableTickers, executionLogs, riskData, _strategyConfigs, futuresParams, totalEquityUsd, tickerVolumes]);

  // Filter by search query (bot-status filter applied separately so the
  // popover counts stay scoped to the search, not to the active filter).
  const searchFilteredTickers = useMemo(() => {
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
    return list;
  }, [tickersWithStats, searchQuery, activeVenueCodes]);

  const botStatusCounts = useMemo(() => {
    const counts: Record<TickerBotStatus | "all", number> = {
      all: searchFilteredTickers.length,
      running: 0,
      paused: 0,
      reducing: 0,
      stopped: 0,
    };
    for (const t of searchFilteredTickers) {
      counts[tickerBotStatus(t.symbol)] += 1;
    }
    return counts;
  }, [searchFilteredTickers, botPhaseVersion]);

  // Filter and sort the tickers
  const processedTickers = useMemo(() => {
    // 1. Filter by bot status
    let list = searchFilteredTickers;
    if (botFilter !== "all") {
      list = list.filter((t) => tickerBotStatus(t.symbol) === botFilter);
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

      if (sortOption === "liquidity") {
        // Avg 24h volume across venues descending; tickers with no reported
        // volume sort last (undefined data never masquerades as zero volume).
        const volA = a.avgVolumeUsd;
        const volB = b.avgVolumeUsd;
        if (volA !== undefined && volB === undefined) return -1;
        if (volA === undefined && volB !== undefined) return 1;
        if (volA !== undefined && volB !== undefined && volA !== volB) {
          return volB - volA;
        }
        return a.symbol.localeCompare(b.symbol);
      }

      return 0;
    });
  }, [searchFilteredTickers, botFilter, sortOption, botPhaseVersion]);

  const BOT_FILTER_OPTIONS: { value: TickerBotStatus | "all"; label: string }[] = [
    { value: "all", label: "All bots" },
    { value: "running", label: "Active" },
    { value: "paused", label: "Paused" },
    { value: "reducing", label: "Reducing" },
    { value: "stopped", label: "Stopped" },
  ];

  // Future context-menu items plug in here — each entry maps to a row below
  // and is handled in handleSelectContextItem(id). Hide keeps current behavior exactly.
  const CONTEXT_MENU_ITEMS: { id: string; label: string; Icon: typeof EyeOff }[] = [
    { id: "hide", label: "Hide", Icon: EyeOff },
  ];

  return (
    <div className="w-full h-full bg-surface border border-surface-border rounded-lg shadow-2xs flex flex-col select-none overflow-hidden">
      {/* Top Header & Search */}
      <div className="p-3 border-b border-surface-border space-y-2.5">
        {/* Search Bar & Sort Configuration Button */}
        <div className="flex items-center space-x-1.5 relative">
          <div className="relative flex-1">
            <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-muted pointer-events-none" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Search perp pairs…"
              aria-label="Search tickers"
              className="w-full pl-8 pr-7 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-sans text-slate-900 placeholder:text-muted focus:outline-none focus:ring-2 focus:ring-primary/15 focus:border-primary transition-all shadow-2xs"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery("")}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-muted hover:text-slate-600 p-0.5 cursor-pointer"
              >
                <X className="w-3 h-3" />
              </button>
            )}
          </div>

          {/* Tiny Bot-Status Filter Button & Popover */}
          <div className="relative" ref={botMenuRef}>
            <button
              onClick={() => setShowBotMenu((prev) => !prev)}
              aria-label="Filter by bot status"
              title={`Bot status filter: ${
                BOT_FILTER_OPTIONS.find((o) => o.value === botFilter)?.label ?? "All bots"
              }`}
              className={`p-1.5 rounded-lg border transition-all flex items-center justify-center relative cursor-pointer shadow-2xs ${
                showBotMenu
                  ? "bg-accent/10 border-accent text-accent-strong"
                  : botFilter !== "all"
                  ? "bg-accent/5 border-accent-line text-accent-strong"
                  : "bg-surface border-surface-border text-slate-600 hover:text-slate-900 hover:border-slate-300"
              }`}
            >
              <Filter className="w-3.5 h-3.5" />
              {botFilter !== "all" && (
                <span className="absolute -top-0.5 -right-0.5 w-1.5 h-1.5 rounded-full bg-accent" />
              )}
            </button>

            {/* Floating Popover Menu */}
            {showBotMenu && (
              <div className="absolute right-0 top-full mt-1.5 w-56 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                <div className="px-3 py-1 text-[11px] font-bold uppercase tracking-wider text-muted border-b border-surface-border/60">
                  Filter by Bot Status
                </div>

                {BOT_FILTER_OPTIONS.map((option) => (
                  <button
                    key={option.value}
                    onClick={() => {
                      setBotFilter(option.value);
                      setShowBotMenu(false);
                    }}
                    className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                      botFilter === option.value
                        ? "bg-accent/10 text-accent-strong font-semibold"
                        : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <span>{option.label}</span>
                      <span className="text-[10px] font-mono tabular-nums text-muted">
                        {botStatusCounts[option.value]}
                      </span>
                    </div>
                    {botFilter === option.value && <Check className="w-3.5 h-3.5 text-accent-strong" />}
                  </button>
                ))}

              </div>
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
                  : sortOption === "liquidity"
                  ? "Avg 24h Volume (Liquidity)"
                  : "Net Profit / Loss"
              }`}
              className={`p-1.5 rounded-lg border transition-all flex items-center justify-center relative cursor-pointer shadow-2xs ${
                showSortMenu
                  ? "bg-accent/10 border-accent text-accent-strong"
                  : sortOption !== "alpha"
                  ? "bg-accent/5 border-accent-line text-accent-strong"
                  : "bg-surface border-surface-border text-slate-600 hover:text-slate-900 hover:border-slate-300"
              }`}
            >
              <ArrowUpDown className="w-3.5 h-3.5" />
              {sortOption !== "alpha" && (
                <span className="absolute -top-0.5 -right-0.5 w-1.5 h-1.5 rounded-full bg-accent" />
              )}
            </button>

            {/* Floating Popover Menu */}
            {showSortMenu && (
              <div className="absolute right-0 top-full mt-1.5 w-56 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                <div className="px-3 py-1 text-[11px] font-bold uppercase tracking-wider text-muted border-b border-surface-border/60">
                  Sort Tickers
                </div>

                <button
                  onClick={() => {
                    setSortOption("alpha");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                    sortOption === "alpha"
                      ? "bg-accent/10 text-accent-strong font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <ArrowDownAZ className="w-3.5 h-3.5 text-slate-500" />
                    <span>A-Z (Name)</span>
                  </div>
                  {sortOption === "alpha" && <Check className="w-3.5 h-3.5 text-accent-strong" />}
                </button>

                <button
                  onClick={() => {
                    setSortOption("trades");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                    sortOption === "trades"
                      ? "bg-accent/10 text-accent-strong font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <Activity className="w-3.5 h-3.5 text-slate-500" />
                    <span>Strategy Trades</span>
                  </div>
                  {sortOption === "trades" && <Check className="w-3.5 h-3.5 text-accent-strong" />}
                </button>

                <button
                  onClick={() => {
                    setSortOption("pnl");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                    sortOption === "pnl"
                      ? "bg-accent/10 text-accent-strong font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <DollarSign className="w-3.5 h-3.5 text-slate-500" />
                    <span>Net Profit / Loss</span>
                  </div>
                  {sortOption === "pnl" && <Check className="w-3.5 h-3.5 text-accent-strong" />}
                </button>

                <button
                  onClick={() => {
                    setSortOption("liquidity");
                    setShowSortMenu(false);
                  }}
                  className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                    sortOption === "liquidity"
                      ? "bg-accent/10 text-accent-strong font-semibold"
                      : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                  }`}
                >
                  <div className="flex items-center space-x-2">
                    <Droplets className="w-3.5 h-3.5 text-slate-600" />
                    <span>Liquidity (Avg 24h Vol)</span>
                  </div>
                  {sortOption === "liquidity" && <Check className="w-3.5 h-3.5 text-accent-strong" />}
                </button>

              </div>
            )}
          </div>
        </div>
      </div>

      {/* Column Subheader */}
      <div className="px-3 py-1.5 border-b border-surface-border bg-surface-raised/60 flex items-center justify-between text-xs font-mono text-slate-600 font-semibold tracking-wider">
        <span>Symbol</span>
        <span className="text-right">Net PnL</span>
      </div>

      {/* Scrollable Ticker List */}
      <div className="flex-1 overflow-y-auto px-2 py-1.5 space-y-1">
        {processedTickers.length === 0 ? (
          searchQuery || botFilter !== "all" ? (
            <EmptyState
              title={<>No tickers matching &quot;{searchQuery}&quot;</>}
              hint="Adjust or clear the search to list all markets."
            />
          ) : (
            <EmptyState
              title="No Available Tickers"
              hint={tickerUnavailableReason ?? tickerUnavailabilityReason(
                systemStatus,
                activeVenueCodes ?? new Set<string>()
              )}
            >
              {onOpenSettings && (
                <button
                  type="button"
                  onClick={onOpenSettings}
                  className="mt-2 inline-flex items-center space-x-1.5 px-2 py-2 text-xs bg-accent text-white rounded-lg font-medium hover:bg-accent-hover transition cursor-pointer shadow-2xs"
                >
                  <Settings className="w-3.5 h-3.5" />
                  <span>Open System Settings</span>
                </button>
              )}
            </EmptyState>
          )
        ) : (
          processedTickers.map((ticker) => (
            <TickerRow
              key={ticker.symbol}
              symbol={ticker.symbol}
              base={ticker.base}
              quote={ticker.quote}
              isSelected={selectedSymbol === ticker.symbol}
              isEnabled={enabledTickers ? enabledTickers.has(ticker.symbol) : false}
              botStatus={tickerBotStatus(ticker.symbol)}
              hasIssue={ticker.hasIssue}
              issueDescription={ticker.issueDescription}
              tradeCount={ticker.tradeCount}
              netProfit={ticker.netProfit}
              netProfitBps={ticker.netProfitBps}
              avgEntrySpreadBps={ticker.avgEntrySpreadBps}
              avgVolumeUsd={ticker.avgVolumeUsd}
              showLiquidity={sortOption === "liquidity"}
              onSelect={onSelectTicker}
              onContextMenu={handleTickerContextMenu}
            />
          ))
        )}
      </div>

      {contextMenu && (
        <div
          ref={contextMenuRef}
          role="menu"
          className="fixed w-52 bg-surface border border-surface-border rounded-lg shadow-lg p-1 z-30 font-mono text-xs animate-in fade-in zoom-in-95 duration-100"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          {CONTEXT_MENU_ITEMS.map(({ id, label, Icon }) => (
            <button
              key={id}
              role="menuitem"
              onClick={() => handleSelectContextItem(id)}
              className="w-full flex items-center space-x-2 px-2 py-2 text-left transition rounded-lg text-slate-700 hover:bg-surface-raised hover:text-slate-900"
            >
              <Icon className="w-3.5 h-3.5 text-slate-600" />
              <span>{label}</span>
            </button>
          ))}
        </div>
      )}

    </div>
  );
};

export const TickerExplorer: React.FC<TickerExplorerProps> = React.memo(TickerExplorerInner);
