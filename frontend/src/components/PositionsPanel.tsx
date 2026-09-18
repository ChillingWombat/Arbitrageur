import React, { useMemo, useRef, useState, useEffect } from "react";
import {
  ArrowDownAZ,
  ArrowUpDown,
  Activity,
  Check,
  DollarSign,
  Layers,
  Settings,
  Sigma,
  TrendingDown,
  Zap,
  Clock,
} from "lucide-react";
import type { OpenPositionDto, SystemStatusDto } from "../types";
import { VENUE_ID_TO_CODE } from "../types/tickers";
import {
  scopeAndSortPositions,
  isPositionReduceOnly,
  setPositionReduceOnly,
  type PositionScope,
  type PositionSortOption,
} from "../services/positionsQuant";
import { api } from "../services/api";
import { EmptyState } from "./EmptyState";

interface PositionsPanelProps {
  positions: OpenPositionDto[];
  /** Strategy id backing the active tab (e.g. "futures"), null when the tab
   *  has no single-strategy scope (portfolio monitor, triangle). */
  currentStrategyId: string | null;
  /** Display label for the current-strategy scope (e.g. "Single Spread"). */
  currentStrategyLabel: string;
  selectedPositionKey: string | null;
  onSelectPosition: (position: OpenPositionDto) => void;
  status?: SystemStatusDto | null;
  isPaused?: boolean;
  onResetCircuitBreaker?: () => void;
  onOpenPortfolioMonitor?: () => void;
  isPortfolioActive?: boolean;
  onOpenSettings?: () => void;
}

const STRATEGY_SHORT: Record<string, string> = {
  futures: "SPREAD",
  radar: "RADAR",
  funding: "FUNDING",
  spot: "SPOT",
  prediction: "PREDICT",
  stat_arb: "STAT ARB",
};

const SCOPE_BUTTON_LABEL: Record<string, string> = {
  prediction: "Prediction",
};

const venueLabel = (venue: string): string =>
  VENUE_ID_TO_CODE[venue.toLowerCase()] ?? venue.toUpperCase().slice(0, 6);

const formatUsd = (v: number): string =>
  `${v < 0 ? "-" : "+"}$${Math.abs(v).toFixed(2)}`;

const formatQty = (q: number): string => {
  const abs = Math.abs(q);
  const digits = abs >= 100 ? 2 : abs >= 1 ? 4 : 6;
  return `${q < 0 ? "-" : "+"}${abs.toFixed(digits)}`;
};

const PositionsPanelInner: React.FC<PositionsPanelProps> = ({
  positions,
  currentStrategyId,
  currentStrategyLabel,
  selectedPositionKey,
  onSelectPosition,
  status,
  isPaused: propIsPaused,
  onResetCircuitBreaker,
  onOpenPortfolioMonitor,
  isPortfolioActive,
  onOpenSettings,
}) => {
  const pnl = status?.total_realized_pnl ?? 0;
  const isTripped = status?.circuit_breaker_tripped ?? false;
  const isPaused = propIsPaused ?? status?.is_paused ?? false;

  const [scope, setScope] = useState<PositionScope>("all");
  const [sortOption, setSortOption] = useState<PositionSortOption>("name");
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
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [showSortMenu]);

  // Tabs with no single-strategy scope can only show the "all" view.
  useEffect(() => {
    if (currentStrategyId === null) setScope("all");
  }, [currentStrategyId]);

  const scoped = useMemo(
    () => scopeAndSortPositions(positions, scope, currentStrategyId, sortOption),
    [positions, scope, currentStrategyId, sortOption],
  );

  // Context menu state on position cards
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    position: OpenPositionDto;
  } | null>(null);
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);
  const [, setReduceOnlyVersion] = useState(0);
  const contextMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleReduceOnlyChanged = () => {
      setReduceOnlyVersion((v) => v + 1);
    };
    window.addEventListener("arbitrageur:reduce_only_changed", handleReduceOnlyChanged);
    return () => window.removeEventListener("arbitrageur:reduce_only_changed", handleReduceOnlyChanged);
  }, []);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (contextMenuRef.current && !contextMenuRef.current.contains(event.target as Node)) {
        setContextMenu(null);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setContextMenu(null);
    };
    if (contextMenu) {
      document.addEventListener("mousedown", handleClickOutside);
      document.addEventListener("keydown", handleKeyDown);
    }
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [contextMenu]);

  const handleContextMenu = (e: React.MouseEvent, pos: OpenPositionDto) => {
    e.preventDefault();
    const menuWidth = 260;
    const menuHeight = 220;
    const x = Math.min(e.clientX, window.innerWidth - menuWidth - 10);
    const y = Math.min(e.clientY, window.innerHeight - menuHeight - 10);
    setContextMenu({ x, y, position: pos });
  };

  const handleToggleReduceOnly = (pos: OpenPositionDto) => {
    const current = isPositionReduceOnly(pos.symbol);
    setPositionReduceOnly(pos.symbol, !current);
    setContextMenu(null);
  };

  const handleClearEmergently = async (pos: OpenPositionDto) => {
    setActionInProgress(`Clearing ${pos.symbol} emergently…`);
    try {
      await api.flattenTickerPositions(pos.symbol, pos.strategyId, "emergency_market");
    } catch (err) {
      console.error("Emergency clear failed", err);
    } finally {
      setActionInProgress(null);
      setContextMenu(null);
    }
  };

  const handleClearNonEmergently = async (pos: OpenPositionDto) => {
    setActionInProgress(`Clearing ${pos.symbol} via limit…`);
    try {
      await api.flattenTickerPositions(pos.symbol, pos.strategyId, "non_emergency_limit");
    } catch (err) {
      console.error("Non-emergency clear failed", err);
    } finally {
      setActionInProgress(null);
      setContextMenu(null);
    }
  };

  return (
    <aside className="w-72 shrink-0 border-r border-surface-border bg-surface flex flex-col justify-between select-none h-full overflow-hidden">
      {/* Header: title + scope toggle + sort */}
      <div className="p-3 border-b border-surface-border space-y-2.5">
        <div className="flex items-center justify-between">
          <div className="text-xs font-sans font-bold uppercase tracking-wider text-slate-900 flex items-center space-x-1.5">
            <span>Positions</span>
            <span className="text-[11px] font-mono font-medium text-slate-500 bg-slate-100 px-1.5 py-0.2 rounded-full">
              ({scoped.length}/{positions.length})
            </span>
          </div>
        </div>

        <div className="flex items-center space-x-1.5">
          {/* Scope toggle: all strategies vs the strategy of the active tab */}
          <div className="flex flex-1 items-center bg-surface-raised border border-surface-border rounded-lg p-0.5 text-xs font-sans">
            <button
              type="button"
              onClick={() => setScope("all")}
              className={`flex-1 px-2 py-2 rounded-lg transition cursor-pointer text-xs ${
                scope === "all"
                  ? "bg-accent text-white shadow-2xs font-semibold"
                  : "text-slate-500 hover:text-slate-900 font-medium"
              }`}
              title="Show open positions across every strategy"
            >
              All
            </button>
            <button
              type="button"
              onClick={() => currentStrategyId && setScope("current")}
              disabled={!currentStrategyId}
              className={`flex-1 px-2 py-2 rounded-lg transition truncate text-xs ${
                scope === "current"
                  ? "bg-accent text-white shadow-2xs font-semibold"
                  : "text-slate-500 hover:text-slate-900 font-medium"
              } ${!currentStrategyId ? "opacity-40 cursor-not-allowed" : "cursor-pointer"}`}
              title={
                currentStrategyId
                  ? `Show only ${currentStrategyLabel} positions`
                  : "This page has no single-strategy scope"
              }
            >
              {(currentStrategyId && SCOPE_BUTTON_LABEL[currentStrategyId]) ||
                currentStrategyLabel}
            </button>
          </div>

          {/* Sort popover */}
          <div className="relative" ref={sortMenuRef}>
            <button
              onClick={() => setShowSortMenu((prev) => !prev)}
              aria-label="Sort positions"
              title={`Sort by: ${
                sortOption === "name"
                  ? "Name (A-Z)"
                  : sortOption === "pnl"
                  ? "Current PnL"
                  : "Position Size"
              }`}
              className={`p-1.5 rounded-lg border transition flex items-center justify-center relative cursor-pointer ${
                showSortMenu
                  ? "bg-accent/10 border-accent text-accent"
                  : sortOption !== "name"
                  ? "bg-accent/5 border-accent-line text-accent"
                  : "bg-surface-raised border-surface-border text-slate-600 hover:text-slate-900 hover:border-slate-300"
              }`}
            >
              <ArrowUpDown className="w-3.5 h-3.5" />
              {sortOption !== "name" && (
                <span className="absolute -top-0.5 -right-0.5 w-1.5 h-1.5 rounded-full bg-accent" />
              )}
            </button>

            {showSortMenu && (
              <div className="absolute right-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 z-30 font-sans text-xs animate-in fade-in zoom-in-95 duration-100">
                <div className="px-3 py-1 text-[11px] font-bold uppercase tracking-wider text-slate-400 border-b border-surface-border/60">
                  Sort Positions
                </div>
                {(
                  [
                    { id: "name", label: "Name (A-Z)", icon: ArrowDownAZ },
                    { id: "pnl", label: "Current PnL", icon: DollarSign },
                    { id: "size", label: "Position Size", icon: Sigma },
                  ] as const
                ).map(({ id, label, icon: Icon }) => (
                  <button
                    key={id}
                    onClick={() => {
                      setSortOption(id);
                      setShowSortMenu(false);
                    }}
                    className={`w-full flex items-center justify-between px-2 py-2 text-left transition rounded-lg cursor-pointer ${
                      sortOption === id
                        ? "bg-accent/10 text-accent font-semibold"
                        : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <Icon className="w-3.5 h-3.5 text-slate-500" />
                      <span>{label}</span>
                    </div>
                    {sortOption === id && <Check className="w-3.5 h-3.5 text-accent" />}
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Column subheader */}
      <div className="px-3 py-1.5 border-b border-surface-border bg-surface-raised/60 flex items-center justify-between text-[11px] font-sans text-slate-500 font-semibold uppercase tracking-wider">
        <span>Position</span>
        <span className="text-right">Venue Net ±</span>
      </div>

      {/* Position cards */}
      <div className="flex-1 overflow-y-auto px-2 py-1.5 space-y-1.5">
        {scoped.length === 0 ? (
          <EmptyState
            icon={Layers}
            title="No Open Positions"
            hint={
              scope === "current"
                ? `The ${currentStrategyLabel} strategy has no open positions right now.`
                : "Executed strategies will aggregate here per pair or event."
            }
          />
        ) : (
          scoped.map((pos) => {
            const isSelected = selectedPositionKey === pos.key;
            const isReduce = isPositionReduceOnly(pos.symbol);
            return (
              <div
                key={pos.key}
                role="button"
                tabIndex={0}
                onClick={() => onSelectPosition(pos)}
                onContextMenu={(e) => handleContextMenu(e, pos)}
                title="Right-click for position menu: Reduce-Only, Emergency Clear, Non-Emergency Clear"
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onSelectPosition(pos);
                  }
                }}
                className={`w-full px-2.5 py-2 rounded-lg border text-left transition cursor-pointer flex items-start justify-between ${
                  isSelected
                    ? "bg-accent-dim/70 border-accent/60 shadow-2xs text-slate-900 border-l-[3px] border-l-accent"
                    : "bg-surface hover:bg-surface-raised border-surface-border text-slate-700 hover:border-slate-300 shadow-2xs"
                }`}
              >
                {/* Left: position identity + PnL below */}
                <div className="flex flex-col min-w-0 pr-2">
                  <div className="flex items-center space-x-1.5 flex-wrap gap-y-0.5">
                    <span className="font-bold text-xs font-sans tracking-tight text-slate-900 truncate">
                      {pos.symbol}
                    </span>
                    {scope === "all" && (
                      <span className="text-[9px] px-1.5 py-0.2 rounded bg-slate-100 text-slate-600 font-mono font-bold border border-slate-200/60 whitespace-nowrap">
                        {STRATEGY_SHORT[pos.strategyId] ?? pos.strategyId.toUpperCase()}
                      </span>
                    )}
                    {isReduce && (
                      <span
                        className="text-[9px] px-1.5 py-0.2 rounded bg-warn-dim text-warn font-mono font-bold border border-warn-border whitespace-nowrap"
                        title="Reduce-Only: No new step increases; position closes on target profit"
                      >
                        Reduce-Only
                      </span>
                    )}
                  </div>
                  <span
                    className="text-xs font-mono text-slate-500 tabular-nums mt-0.5"
                    title={
                      pos.unrealizedPnlUsd !== null
                        ? `Mark-to-market PnL net of $${pos.feesUsd.toFixed(2)} fees`
                        : pos.expectedNetUsd !== null
                        ? `Mark-to-market unavailable; entry-time expected net ${formatUsd(pos.expectedNetUsd)}`
                        : "No mark-to-market data"
                    }
                  >
                    PnL {pos.unrealizedPnlUsd !== null ? formatUsd(pos.unrealizedPnlUsd) : "—"}
                  </span>
                </div>

                {/* Right: per-venue signed net quantities — buy top, sell bottom */}
                <div className="flex flex-col items-end space-y-0.5 shrink-0 font-mono">
                  {pos.venues.map((v) => (
                    <div key={v.venue} className="flex items-baseline space-x-1.5 text-xs">
                      <span className="text-slate-400 font-mono text-[11px]">{venueLabel(v.venue)}</span>
                      <span
                        className={`tabular-nums font-semibold ${
                          v.netQty > 0
                            ? "text-slate-800"
                            : v.netQty < 0
                            ? "text-slate-800"
                            : "text-slate-400"
                        }`}
                      >
                        {formatQty(v.netQty)}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Bottom Control Panel: Net Profit, Risk Indicator & Settings */}
      <div className="p-3 border-t border-surface-border bg-surface-raised space-y-2.5">
        <div className="flex items-center justify-between">
          <div className="flex flex-col">
            <span className="text-[11px] font-sans uppercase tracking-wider text-slate-500 font-semibold">
              Net Profit
            </span>
            <span
              className={`font-mono text-xs font-bold tabular-nums ${
                pnl >= 0 ? "text-profit" : "text-loss"
              }`}
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          <div className="flex items-center space-x-1.5">
            <div
              className={`px-2.5 py-1 rounded-full border flex items-center space-x-1.5 text-[11px] font-sans font-semibold ${
                isTripped
                  ? "bg-loss-dim border-loss-border text-loss"
                  : isPaused
                  ? "bg-warn-dim border-warn-border text-warn"
                  : "bg-profit-dim border-profit-border text-profit"
              }`}
              title={
                isTripped
                  ? "Circuit Breaker Tripped: Risk limits breached or Emergency Stop triggered"
                  : isPaused
                  ? "Status: Paused (Executions suspended, positions preserved)"
                  : "Risk: Normal (Margin, drawdown, and delta neutrality healthy)"
              }
            >
              <span
                className={`w-1.5 h-1.5 rounded-full ${
                  isTripped
                    ? "bg-loss animate-pulse"
                    : isPaused
                    ? "bg-warn animate-pulse"
                    : "bg-profit"
                }`}
              />
              <span>
                {isTripped ? "CB: Tripped" : isPaused ? "Status: Paused" : "Risk: Normal"}
              </span>
            </div>

            {isTripped && onResetCircuitBreaker && (
              <button
                type="button"
                onClick={onResetCircuitBreaker}
                className="btn-warning-subtle text-xs font-mono font-bold px-2 py-2"
                title="Reset Circuit Breaker"
              >
                Reset
              </button>
            )}
          </div>
        </div>

        <button
          type="button"
          onClick={onOpenPortfolioMonitor}
          className={`w-full py-2 px-2 rounded-lg text-xs font-sans font-semibold transition-all shadow-2xs flex items-center justify-center space-x-2 cursor-pointer border active:scale-[0.98] ${
            isPortfolioActive
              ? "bg-accent text-white border-accent shadow-xs font-bold"
              : "bg-surface hover:bg-slate-50 text-slate-800 hover:text-slate-900 border-surface-border hover:border-slate-300"
          }`}
          title="Open Portfolio Monitor (Consolidated DeltaGuard, Margins &amp; Strategy Performance)"
        >
          <Activity className={`w-3.5 h-3.5 ${isPortfolioActive ? "text-white" : "text-accent"}`} />
          <span>Portfolio Monitor</span>
        </button>

        <button
          type="button"
          onClick={onOpenSettings}
          className="w-full py-2 px-2 bg-surface hover:bg-slate-50 text-slate-700 hover:text-slate-900 border border-surface-border hover:border-slate-300 rounded-lg text-xs font-sans font-medium transition-all shadow-2xs flex items-center justify-center space-x-2 cursor-pointer active:scale-[0.98]"
          title="System Setting &amp; Exchange API Keys"
        >
          <Settings className="w-3.5 h-3.5 text-slate-500" />
          <span>System Setting</span>
        </button>
      </div>

      {/* Right-click Context Menu on Position Card */}
      {contextMenu && (
        <div
          ref={contextMenuRef}
          style={{ top: contextMenu.y, left: contextMenu.x }}
          className="fixed z-50 w-64 bg-surface border border-surface-border rounded-xl shadow-elevated p-1.5 text-xs font-sans animate-in fade-in zoom-in-95 duration-100"
        >
          {/* Header */}
          <div className="px-3 py-1.5 border-b border-surface-border flex items-center justify-between text-[11px] font-mono font-bold text-slate-700">
            <span className="truncate">{contextMenu.position.symbol}</span>
            <span className="text-slate-400 font-normal uppercase text-[10px]">
              {STRATEGY_SHORT[contextMenu.position.strategyId] ?? contextMenu.position.strategyId}
            </span>
          </div>

          <div className="py-1 space-y-0.5">
            {/* Option 1: Reduce Only */}
            <button
              type="button"
              onClick={() => handleToggleReduceOnly(contextMenu.position)}
              className="w-full flex items-start space-x-2.5 px-2 py-2 text-left rounded-lg transition hover:bg-warn-dim/40 cursor-pointer group"
            >
              <TrendingDown className="w-4 h-4 text-warn shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <div className="font-semibold text-slate-800 group-hover:text-slate-900 flex items-center justify-between">
                  <span>Reduce Only</span>
                  {isPositionReduceOnly(contextMenu.position.symbol) && (
                    <span className="text-[10px] text-warn font-mono font-bold">Active</span>
                  )}
                </div>
                <p className="text-[10px] text-slate-500 mt-0.5 leading-tight">
                  Bot continues monitoring &amp; exits on target, but halts further step expansion.
                </p>
              </div>
            </button>

            {/* Option 2: Clear Emergently (Market Order) */}
            <button
              type="button"
              disabled={actionInProgress !== null}
              onClick={() => void handleClearEmergently(contextMenu.position)}
              className="w-full flex items-start space-x-2.5 px-2 py-2 text-left rounded-lg transition hover:bg-loss-dim cursor-pointer group disabled:opacity-50"
            >
              <Zap className="w-4 h-4 text-loss shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <div className="font-semibold text-loss flex items-center justify-between">
                  <span>Clear Emergently</span>
                  <span className="text-[9px] font-mono uppercase bg-loss/10 text-loss px-1 rounded">Market</span>
                </div>
                <p className="text-[10px] text-slate-500 mt-0.5 leading-tight">
                  Immediate market order liquidation across all legs for this position.
                </p>
              </div>
            </button>

            {/* Option 3: Clear Non-Emergently (Limit -> Market Fallback) */}
            <button
              type="button"
              disabled={actionInProgress !== null}
              onClick={() => void handleClearNonEmergently(contextMenu.position)}
              className="w-full flex items-start space-x-2.5 px-2 py-2 text-left rounded-lg transition hover:bg-blue-50/70 cursor-pointer group disabled:opacity-50"
            >
              <Clock className="w-4 h-4 text-accent shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <div className="font-semibold text-slate-800 group-hover:text-slate-900 flex items-center justify-between">
                  <span>Clear Non-Emergently</span>
                  <span className="text-[9px] font-mono uppercase bg-accent/10 text-accent px-1 rounded">Limit</span>
                </div>
                <p className="text-[10px] text-slate-500 mt-0.5 leading-tight">
                  Attempts passive limit pegged inside spread first, falling back to market on timeout.
                </p>
              </div>
            </button>
          </div>

          {actionInProgress && (
            <div className="p-2 border-t border-surface-border text-center text-slate-500 text-[11px] font-mono animate-pulse">
              {actionInProgress}
            </div>
          )}
        </div>
      )}
    </aside>
  );
};

export const PositionsPanel: React.FC<PositionsPanelProps> = React.memo(PositionsPanelInner);
