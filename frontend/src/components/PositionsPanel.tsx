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
} from "lucide-react";
import type { OpenPositionDto, SystemStatusDto } from "../types";
import { VENUE_ID_TO_CODE } from "../types/tickers";
import {
  scopeAndSortPositions,
  type PositionScope,
  type PositionSortOption,
} from "../services/positionsQuant";
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

  return (
    <aside className="w-72 shrink-0 border-r border-surface-border bg-surface flex flex-col justify-between select-none h-full overflow-hidden">
      {/* Header: title + scope toggle + sort */}
      <div className="p-3 border-b border-surface-border space-y-2.5">
        <div className="flex items-center justify-between">
          <div className="text-[13px] font-mono font-bold tracking-wider text-slate-900 flex items-center space-x-1.5">
            <span>Positions</span>
            <span className="text-xs font-normal text-slate-600">
              ({scoped.length}/{positions.length})
            </span>
          </div>
        </div>

        <div className="flex items-center space-x-1.5">
          {/* Scope toggle: all strategies vs the strategy of the active tab */}
          <div className="flex flex-1 items-center bg-surface-raised border border-surface-border rounded-md p-0.5 text-[11px] font-mono">
            <button
              type="button"
              onClick={() => setScope("all")}
              className={`flex-1 px-2 py-1 rounded transition cursor-pointer ${
                scope === "all"
                  ? "bg-accent text-white shadow-2xs font-bold"
                  : "text-slate-500 hover:text-slate-900"
              }`}
              title="Show open positions across every strategy"
            >
              All
            </button>
            <button
              type="button"
              onClick={() => currentStrategyId && setScope("current")}
              disabled={!currentStrategyId}
              className={`flex-1 px-2 py-1 rounded transition truncate ${
                scope === "current"
                  ? "bg-accent text-white shadow-2xs font-bold"
                  : "text-slate-500 hover:text-slate-900"
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
              className={`p-1.5 rounded-md border transition flex items-center justify-center relative ${
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
              <div className="absolute right-0 top-full mt-1.5 w-52 bg-surface border border-surface-border rounded-md shadow-lg p-1 z-30 font-mono text-xs animate-in fade-in zoom-in-95 duration-100">
                <div className="px-3.5 py-1 -mx-1 text-xs font-bold text-slate-600 border-b border-surface-border/60">
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
                    className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition rounded-sm ${
                      sortOption === id
                        ? "bg-accent/10 text-accent font-semibold"
                        : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <Icon className="w-3.5 h-3.5 text-slate-600" />
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
      <div className="px-3 py-1.5 border-b border-surface-border bg-surface-raised/60 flex items-center justify-between text-xs font-mono text-slate-600 font-semibold tracking-wider">
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
            return (
              <div
                key={pos.key}
                role="button"
                tabIndex={0}
                onClick={() => onSelectPosition(pos)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onSelectPosition(pos);
                  }
                }}
                className={`w-full px-2.5 py-2 rounded-lg border text-left transition font-mono cursor-pointer flex items-start justify-between ${
                  isSelected
                    ? "bg-accent-dim/70 border-accent/60 shadow-2xs text-slate-900 border-l-[3px] border-l-accent"
                    : "bg-surface hover:bg-surface-raised border-surface-border text-slate-700 hover:border-slate-300 shadow-2xs"
                }`}
              >
                {/* Left: position identity + PnL below */}
                <div className="flex flex-col min-w-0 pr-2">
                  <div className="flex items-center space-x-1.5">
                    <span className="font-bold text-xs tracking-tight text-slate-900 truncate">
                      {pos.symbol}
                    </span>
                    {scope === "all" && (
                      <span className="text-[8px] px-1 py-0.5 rounded-sm bg-slate-200/70 text-slate-600 font-semibold whitespace-nowrap">
                        {STRATEGY_SHORT[pos.strategyId] ?? pos.strategyId.toUpperCase()}
                      </span>
                    )}
                  </div>
                  <span
                    className="text-xs text-slate-500 tabular-nums mt-0.5"
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
                <div className="flex flex-col items-end space-y-0.5 shrink-0">
                  {pos.venues.map((v) => (
                    <div key={v.venue} className="flex items-baseline space-x-1.5 text-xs">
                      <span className="text-slate-500">{venueLabel(v.venue)}</span>
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
            <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">
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

          <div className="flex items-center space-x-1.5">
            <div
              className={`px-2.5 py-1 rounded-md border flex items-center space-x-1.5 text-xs font-mono font-medium ${
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
                className="px-1.5 py-1 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-md text-xs font-mono font-medium transition cursor-pointer"
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
          className={`w-full py-2 px-3 rounded-md text-xs font-mono font-medium transition shadow-2xs flex items-center justify-center space-x-2 cursor-pointer border ${
            isPortfolioActive
              ? "bg-accent text-white border-accent shadow-2xs font-bold"
              : "bg-surface hover:bg-slate-100 text-slate-800 hover:text-slate-900 border-surface-border"
          }`}
          title="Open Portfolio Monitor (Consolidated DeltaGuard, Margins & Strategy Performance)"
        >
          <Activity className={`w-3.5 h-3.5 ${isPortfolioActive ? "text-white" : "text-accent"}`} />
          <span>Portfolio Monitor</span>
        </button>

        <button
          type="button"
          onClick={onOpenSettings}
          className="w-full py-1.5 px-3 bg-surface hover:bg-slate-100 text-slate-700 hover:text-slate-900 border border-surface-border rounded-md text-xs font-mono font-medium transition shadow-2xs flex items-center justify-center space-x-2 cursor-pointer"
          title="System Setting & Exchange API Keys"
        >
          <Settings className="w-3.5 h-3.5 text-slate-500" />
          <span>System Setting</span>
        </button>
      </div>
    </aside>
  );
};

export const PositionsPanel: React.FC<PositionsPanelProps> = React.memo(PositionsPanelInner);
