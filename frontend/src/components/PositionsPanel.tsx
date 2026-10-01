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

import {
  scopeAndSortPositions,
  type PositionScope,
  type PositionSortOption,
} from "../services/positionsQuant";
import { api, describeCommandError } from "../services/api";
import { EmptyState } from "./EmptyState";
import { VenueLogo } from "./VenueLogo";
import { PositionDetailModal } from "./PositionDetailModal";
import { ConfirmActionModal, type ConfirmActionSpec } from "./ConfirmActionModal";

interface PositionsPanelProps {
  positions: OpenPositionDto[];
  /** Strategy id backing the active tab (e.g. "futures"), null when the tab
   *  has no single-strategy scope (portfolio monitor, triangle). */
  currentStrategyId: string | null;
  /** Display label for the current-strategy scope (e.g. "Simple Spread"). */
  currentStrategyLabel: string;
  selectedPositionKey: string | null;
  onSelectPosition: (position: OpenPositionDto) => void;
  status?: SystemStatusDto | null;
  isPaused?: boolean;
  onOpenPortfolioMonitor?: () => void;
  isPortfolioActive?: boolean;
  onOpenSettings?: () => void;
  /** Refetch everything after a backend state change (reduce-only, flatten). */
  onRefreshAll?: () => Promise<void>;
  /** Command failures surface as toasts — never swallowed to console. */
  onCommandError?: (message: string) => void;
}

const STRATEGY_SHORT: Record<string, string> = {
  futures: "SPREAD",
  radar: "SPREAD",
  funding: "FUNDING RATE",
  spot: "SPOT",
  prediction: "PREDICT",
  stat_arb: "STAT ARB",
  options_parity: "PARITY",
  options_jelly_roll: "JELLY ROLL",
  options_box_spread: "BOX",
  options_convexity: "CONVEXITY",
};

const SCOPE_BUTTON_LABEL: Record<string, string> = {
  prediction: "Prediction",
};

const formatUsd = (v: number): string =>
  `${v < 0 ? "-" : "+"}$${Math.abs(v).toFixed(2)}`;

export const formatQty = (q: number): string => {
  const abs = Math.abs(q);
  const digits = abs >= 100 ? 2 : 4;
  const trimmed = abs.toFixed(digits).replace(/\.?0+$/, "");
  return `${q < 0 ? "-" : "+"}${trimmed === "" ? "0" : trimmed}`;
};

const PositionsPanelInner: React.FC<PositionsPanelProps> = ({
  positions,
  currentStrategyId,
  currentStrategyLabel,
  selectedPositionKey,
  onSelectPosition,
  status,
  isPaused: propIsPaused,
  onOpenPortfolioMonitor,
  isPortfolioActive,
  onOpenSettings,
  onRefreshAll,
  onCommandError,
}) => {
  const pnl = status?.total_realized_pnl ?? 0;
  // No status snapshot means the trading state is UNKNOWN — not halted,
  // not healthy. The badge renders "Unknown" rather than defaulting to a
  // healthy claim with no data behind it.
  const statusKnown = status != null;
  const isTripped = status?.circuit_breaker_tripped ?? false;
  const isStopped = status?.is_stopped ?? (isTripped && status?.circuit_breaker_status?.includes("KILLSWITCH")) ?? false;
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

  // Left-click detail modal
  const [detailPos, setDetailPos] = useState<OpenPositionDto | null>(null);

  // Context menu state on position cards
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    position: OpenPositionDto;
  } | null>(null);
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmActionSpec | null>(null);
  const pendingFlatten = useRef<{ pos: OpenPositionDto; mode: "emergency_market" | "non_emergency_limit" } | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);

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

  const handleToggleReduceOnly = async (pos: OpenPositionDto) => {
    // Durable server-side flag — enforced by every backend entry gate, so a
    // manual execute can never bypass it the way the localStorage flag could.
    // Enabling flags the trade's display label; disabling clears that label
    // plus any leg-instrument flags so a stale per-leg flag can't keep the
    // card badged after the operator turns it off.
    try {
      await api.setTickerReduceOnly(pos.symbol, pos.strategyId, !pos.reduceOnly);
      if (pos.reduceOnly) {
        for (const leg of pos.legSymbols) {
          if (leg !== pos.symbol) {
            await api.setTickerReduceOnly(leg, pos.strategyId, false);
          }
        }
      }
      await onRefreshAll?.();
    } catch (err) {
      onCommandError?.(describeCommandError(err, "Reduce-only update failed"));
    } finally {
      setContextMenu(null);
    }
  };

  const requestClear = (pos: OpenPositionDto, mode: "emergency_market" | "non_emergency_limit") => {
    const emergency = mode === "emergency_market";
    const legCount = pos.venues.length;
    setConfirm({
      title: emergency ? "Emergency Clear" : "Clear Position",
      subtitle: `${pos.symbol} · ${pos.strategyId}${legCount > 1 ? ` · ${legCount} legs` : ""}`,
      body: emergency
        ? `This dispatches market-close orders for every open leg of this ${pos.symbol} trade at the prevailing venue price — immediate fill, whatever the slippage. Continue?`
        : `This dispatches limit-close orders for every open leg of this ${pos.symbol} trade. Unfilled residuals stay open and re-escalate — verify flatness afterward.`,
      confirmLabel: emergency ? "Clear at Market" : "Clear via Limit",
    });
    pendingFlatten.current = { pos, mode };
    setContextMenu(null);
  };

  const runConfirmedFlatten = async () => {
    const pending = pendingFlatten.current;
    if (!pending) return;
    const { pos, mode } = pending;
    setActionInProgress(
      mode === "emergency_market"
        ? `Clearing ${pos.symbol} emergently…`
        : `Clearing ${pos.symbol} via limit…`
    );
    try {
      await api.flattenTradePositions(pos.tradeId, pos.strategyId, mode);
      await onRefreshAll?.();
    } catch (err) {
      onCommandError?.(describeCommandError(err, `Clear ${pos.symbol} failed`));
    } finally {
      setActionInProgress(null);
    }
  };

  return (
    <aside className="w-72 shrink-0 border-r border-surface-border bg-surface flex flex-col justify-between select-none h-full overflow-hidden">
      {/* Header: title + scope toggle + sort */}
      <div className="p-3 border-b border-surface-border space-y-2.5">
        <div className="flex items-center justify-between">
          <div className="text-xs font-sans font-bold uppercase tracking-wider text-slate-900 flex items-center space-x-1.5">
            <span>Positions</span>
            <span className="text-[11px] font-mono font-medium text-slate-500 bg-slate-100 px-1.5 py-px rounded-full">
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
                  ? "bg-accent/10 border-accent text-accent-strong"
                  : sortOption !== "name"
                  ? "bg-accent/5 border-accent-line text-accent-strong"
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
                <div className="px-3 py-1 text-[11px] font-bold uppercase tracking-wider text-muted border-b border-surface-border/60">
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
                        ? "bg-accent/10 text-accent-strong font-semibold"
                        : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                    }`}
                  >
                    <div className="flex items-center space-x-2">
                      <Icon className="w-3.5 h-3.5 text-slate-500" />
                      <span>{label}</span>
                    </div>
                    {sortOption === id && <Check className="w-3.5 h-3.5 text-accent-strong" />}
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
        <span className="text-right">Quantity</span>
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
            const isReduce = pos.reduceOnly;
            const multiLeg = pos.venues.length > 1;
            // Every card collapses legs to per-(venue, side) totals — venue
            // name on the left, signed net on the right (the sign IS the
            // side). Instrument-level detail lives in the details modal.
            const venueRows = (() => {
              const acc = new Map<string, { venue: string; qty: number; side: "long" | "short" }>();
              for (const v of pos.venues) {
                if (v.netQty === 0) continue;
                const side = v.netQty > 0 ? ("long" as const) : ("short" as const);
                const key = `${v.venue}|${side}`;
                const r = acc.get(key) ?? { venue: v.venue, qty: 0, side };
                r.qty += v.netQty;
                acc.set(key, r);
              }
              return [...acc.values()].sort(
                (a, b) => a.venue.localeCompare(b.venue) || (a.side === "long" ? -1 : 1),
              );
            })();
            const shownRows = venueRows.slice(0, 4);
            const hiddenRows = venueRows.length - shownRows.length;
            return (
              <div
                key={pos.key}
                role="button"
                tabIndex={0}
                onClick={() => {
                  onSelectPosition(pos);
                  setDetailPos(pos);
                }}
                onContextMenu={(e) => handleContextMenu(e, pos)}
                title="Left-click: position details · Right-click: Reduce-Only / Clear actions"
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onSelectPosition(pos);
                    setDetailPos(pos);
                  }
                }}
                className={`w-full px-2.5 py-2 rounded-lg border text-left transition cursor-pointer ${
                  isSelected
                    ? "bg-accent-dim/70 border-accent/60 shadow-2xs text-slate-900 border-l-[3px] border-l-accent"
                    : "bg-surface hover:bg-surface-raised border-surface-border text-slate-700 hover:border-slate-300 shadow-2xs"
                }`}
              >
                {/* Header: trade label + leg-count / reduce-only flags. */}
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center space-x-1.5 flex-wrap gap-y-0.5 min-w-0">
                    <span className="font-bold text-xs font-sans tracking-tight text-slate-900 truncate">
                      {pos.symbol}
                    </span>
                    {multiLeg && (
                      <span
                        className="text-[9px] px-1.5 py-px rounded bg-slate-100 text-slate-500 font-mono font-bold border border-slate-200/60 whitespace-nowrap"
                        title={`${pos.venues.length} leg instruments in this trade — open details for the full breakdown`}
                      >
                        {pos.venues.length} legs
                      </span>
                    )}
                    {isReduce && (
                      <span
                        className="text-[9px] px-1.5 py-px rounded bg-warn-dim text-warn-strong font-mono font-bold border border-warn-border whitespace-nowrap"
                        title="Reduce-Only: No new step increases; position closes on target profit"
                      >
                        Reduce-Only
                      </span>
                    )}
                  </div>
                </div>

                {/* Per-(venue, side) totals for every strategy — venue name
                    on the left, signed net on the right; instruments live in
                    the details modal. */}
                <div className="mt-1 space-y-px font-mono">
                  {shownRows.map((r) => (
                    <div
                      key={`${r.venue}|${r.side}`}
                      className="flex items-baseline justify-between gap-2 text-[10px]"
                    >
                      <span className="inline-flex items-center gap-1.5 text-slate-600 font-sans font-semibold min-w-0">
                        <VenueLogo venueId={r.venue.toLowerCase()} name={r.venue} size={13} />
                        <span className="truncate">{r.venue}</span>
                      </span>
                      <span
                        className={`tabular-nums font-semibold shrink-0 ${
                          r.qty >= 0 ? "text-emerald-700" : "text-rose-700"
                        }`}
                      >
                        {formatQty(r.qty)}
                      </span>
                    </div>
                  ))}
                  {venueRows.length === 0 && (
                    <div className="text-[9px] text-muted font-sans">no net exposure</div>
                  )}
                  {hiddenRows > 0 && (
                    <div className="text-[9px] text-muted font-sans">
                      +{hiddenRows} more venue row{hiddenRows === 1 ? "" : "s"} — open details
                    </div>
                  )}
                </div>

                {/* Footer: strategy chip left, net-of-fees PnL right. */}
                <div className="mt-1.5 pt-1 border-t border-surface-border/60 flex items-center justify-between gap-2">
                  <span className="text-[9px] px-1.5 py-px rounded bg-slate-100 text-slate-600 font-mono font-bold border border-slate-200/60 whitespace-nowrap">
                    {STRATEGY_SHORT[pos.strategyId] ?? pos.strategyId.toUpperCase()}
                  </span>
                  <span
                    className={`text-[10px] font-semibold tabular-nums shrink-0 ${
                      pos.unrealizedPnlUsd === null
                        ? "text-slate-500"
                        : pos.unrealizedPnlUsd >= 0
                        ? "text-emerald-600"
                        : "text-rose-600"
                    }`}
                    title={
                      pos.unrealizedPnlUsd !== null
                        ? `Current Net Profit (mark-to-market net of $${pos.feesUsd.toFixed(2)} fees)`
                        : pos.expectedNetUsd !== null
                        ? `Mark-to-market unavailable; entry expected net ${formatUsd(pos.expectedNetUsd)}`
                        : "No mark-to-market data"
                    }
                  >
                    {pos.unrealizedPnlUsd !== null ? formatUsd(pos.unrealizedPnlUsd) : "—"}
                  </span>
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
              title="Realized trading PnL — closed-leg deltas net of fees."
            >
              {pnl >= 0 ? `+$${pnl.toFixed(2)}` : `-$${Math.abs(pnl).toFixed(2)}`}
            </span>
          </div>

          <div className="flex items-center space-x-1.5">
            <div
              className={`px-2.5 py-1 rounded-full border flex items-center space-x-1.5 text-[11px] font-sans font-semibold ${
                !statusKnown
                  ? "bg-surface-hover border-border text-slate-500"
                  : isStopped
                  ? "bg-surface-raised border-surface-border text-slate-500"
                  : isTripped
                  ? "bg-loss-dim border-loss-border text-loss"
                  : isPaused
                  ? "bg-warn-dim border-warn-border text-warn-strong"
                  : "bg-profit-dim border-profit-border text-profit"
              }`}
              title={
                !statusKnown
                  ? "Unknown — no backend status snapshot loaded yet"
                  : isStopped
                  ? "Stopped — emergency exit executed via Stop All. All positions cleared."
                  : isTripped
                  ? "Halted — circuit breaker tripped: risk limits breached or Emergency Stop triggered. Reset in Portfolio Monitor."
                  : isPaused
                  ? "Paused — executions suspended, positions preserved"
                  : "Healthy — margin, drawdown, and delta neutrality ok"
              }
            >
              <span
                className={`w-1.5 h-1.5 rounded-full ${
                  !statusKnown
                    ? "bg-slate-400"
                    : isStopped
                    ? "bg-slate-400"
                    : isTripped
                    ? "bg-loss animate-pulse"
                    : isPaused
                    ? "bg-warn animate-pulse"
                    : "bg-profit"
                }`}
              />
              <span>
                {!statusKnown ? "Unknown" : isStopped ? "Stopped" : isTripped ? "Halted" : isPaused ? "Paused" : "Healthy"}
              </span>
            </div>
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
          <Activity className={`w-3.5 h-3.5 ${isPortfolioActive ? "text-white" : "text-accent-strong"}`} />
          <span>Portfolio Monitor</span>
        </button>

        <button
          type="button"
          onClick={onOpenSettings}
          className="w-full py-2 px-2 bg-surface hover:bg-slate-50 text-slate-700 hover:text-slate-900 border border-surface-border hover:border-slate-300 rounded-lg text-xs font-sans font-medium transition-all shadow-2xs flex items-center justify-center space-x-2 cursor-pointer active:scale-[0.98]"
          title="System Settings &amp; Exchange API Keys"
        >
          <Settings className="w-3.5 h-3.5 text-slate-500" />
          <span>System Settings</span>
        </button>
      </div>

      {/* Left-click Position Details */}
      {detailPos && (
        <PositionDetailModal pos={detailPos} onClose={() => setDetailPos(null)} />
      )}

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
            <span className="text-muted font-normal uppercase text-[10px]">
              {STRATEGY_SHORT[contextMenu.position.strategyId] ?? contextMenu.position.strategyId}
            </span>
          </div>

          <div className="py-1 space-y-0.5">
            {/* Option 1: Reduce Only */}
            <button
              type="button"
              onClick={() => void handleToggleReduceOnly(contextMenu.position)}
              className="w-full flex items-start space-x-2.5 px-2 py-2 text-left rounded-lg transition hover:bg-warn-dim/40 cursor-pointer group"
            >
              <TrendingDown className="w-4 h-4 text-warn-strong shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <div className="font-semibold text-slate-800 group-hover:text-slate-900 flex items-center justify-between">
                  <span>Reduce Only</span>
                  {contextMenu.position.reduceOnly && (
                    <span className="text-[10px] text-warn-strong font-mono font-bold">Active</span>
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
              onClick={() => requestClear(contextMenu.position, "emergency_market")}
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
              onClick={() => requestClear(contextMenu.position, "non_emergency_limit")}
              className="w-full flex items-start space-x-2.5 px-2 py-2 text-left rounded-lg transition hover:bg-blue-50/70 cursor-pointer group disabled:opacity-50"
            >
              <Clock className="w-4 h-4 text-accent-strong shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <div className="font-semibold text-slate-800 group-hover:text-slate-900 flex items-center justify-between">
                  <span>Clear Non-Emergently</span>
                  <span className="text-[9px] font-mono uppercase bg-accent/10 text-accent-strong px-1 rounded">Limit</span>
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

      <ConfirmActionModal
        spec={confirm}
        onConfirm={runConfirmedFlatten}
        onCancel={() => {
          setConfirm(null);
          pendingFlatten.current = null;
        }}
        onError={(msg) => onCommandError?.(msg)}
      />
    </aside>
  );
};

export const PositionsPanel: React.FC<PositionsPanelProps> = React.memo(PositionsPanelInner);
