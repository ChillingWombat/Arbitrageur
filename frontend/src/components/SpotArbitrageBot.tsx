import React, { useState, useEffect, useRef, useMemo } from "react";
import {
  Zap,
  AlertCircle,
  Clock,
  SlidersHorizontal,
  ChevronDown,
  Check,
  Search,
  Wallet,
  History,
  Radar,
  Terminal,
  X,
} from "lucide-react";
import { api } from "../services/api";
import type {
  SpotOpportunityDto,
  SpotExecutionParams,
  ExecutionLogEntry,
  StrategyCapabilityDto,
  OpenPositionDto,
  TradedOpportunityDto,
} from "../types";
import type { SpotStrategyParams } from "./TickerConfigModal";
import { BotLifecycleControls, BotPhaseBadge } from "./BotLifecycleControls";
import { EmptyState } from "./EmptyState";
import { ExecuteButton } from "./ExecuteButton";
import { CapacityChart } from "./CapacityChart";
import { InfoTooltip } from "./InfoTooltip";
import { useBotLifecycle } from "../hooks/useBotLifecycle";
import { TickerTransactionHistory } from "./TickerTransactionHistory";
import { TickerLogMonitor } from "./TickerLogMonitor";
import {
  TradedOpportunitiesTable,
  TradedOpportunityDetailModal,
} from "./TradedOpportunitiesGadget";
import { PositionDetailModal } from "./PositionDetailModal";

interface SpotArbitrageBotProps {
  executionLogs: ExecutionLogEntry[];
  onRefreshAll: () => Promise<void>;
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  params: SpotStrategyParams;
  /** Backend capability descriptor — gates the whole page off when the
   *  backend reports the spot path unwired. */
  capability?: StrategyCapabilityDto;
  openPositions?: OpenPositionDto[];
}

export const SpotArbitrageBot: React.FC<SpotArbitrageBotProps> = ({
  executionLogs,
  onRefreshAll,
  onOpenConfig,
  isPaused = false,
  hasApiConnectivity = false,
  params,
  capability,
  openPositions = [],
}) => {
  const [opportunities, setOpportunities] = useState<SpotOpportunityDto[]>([]);
  const [assetFilters, setAssetFilters] = useState<string[]>([]);
  const [showAssetMenu, setShowAssetMenu] = useState(false);
  const assetMenuRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(false);
  const [selectedOpp, setSelectedOpp] = useState<SpotOpportunityDto | null>(null);
  const [search, setSearch] = useState("");
  const [showStageModal, setShowStageModal] = useState(false);
  const [dockTab, setDockTab] = useState<"position" | "history" | "opps" | "logs">("history");
  const [tradedOpps, setTradedOpps] = useState<TradedOpportunityDto[] | null>(null);
  const [tradedFailed, setTradedFailed] = useState(false);
  const [selectedTradedOpp, setSelectedTradedOpp] = useState<TradedOpportunityDto | null>(null);
  const [selectedDetailPos, setSelectedDetailPos] = useState<OpenPositionDto | null>(null);

  // Bot lifecycle: Start / Pause / Reduce Only / Stop (stop flattens this
  // strategy's positions across venues until delta-neutral)
  const bot = useBotLifecycle({ strategyId: "spot" });
  const lastAutoSpotExecRef = React.useRef<number>(0);
  // Strategy params are owned by App and edited only via the Strategy Configurator
  const { tradeSizeUsd, minNetSpreadBps, maxSlippageBps } = params;
  const [botStatus, setBotStatus] = useState<"idle" | "verifying" | "dispatching" | "filled" | "error">("idle");
  const [botMessage, setBotMessage] = useState<string>("");

  // Asset filter derives from what venues actually list — spot covers any
  // asset class an admitted venue exposes (crypto, equities), not a fixed set.
  const assetBase = (sym: string) => sym.split(/[-_/]/)[0]?.toUpperCase() ?? sym;
  const assets = React.useMemo(
    () => Array.from(new Set(opportunities.map((o) => assetBase(o.symbol)))).sort(),
    [opportunities],
  );
  const filteredOpportunities = useMemo(() => {
    let out = opportunities;
    if (assetFilters.length > 0) {
      out = out.filter((o) => assetFilters.includes(assetBase(o.symbol)));
    }
    if (search.trim() !== "") {
      const q = search.trim().toUpperCase();
      out = out.filter((o) => o.symbol.toUpperCase().includes(q));
    }
    return out;
  }, [opportunities, assetFilters, search]);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const rows = await api.listTradedOpportunities();
        if (!alive) return;
        const filtered = Array.isArray(rows)
          ? rows.filter((r) => {
              const s = (r.strategyId ?? "").toLowerCase();
              return s === "spot" || s === "spatial";
            })
          : [];
        setTradedOpps(filtered);
        setTradedFailed(false);
      } catch {
        if (!alive) return;
        setTradedFailed(true);
      }
    };
    void load();
    const interval = setInterval(load, 5000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, []);

  const spotPositions = useMemo(
    () =>
      openPositions.filter((p) => {
        const s = (p.strategyId ?? "").toLowerCase();
        return s === "spot" || s === "spatial";
      }),
    [openPositions]
  );

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (assetMenuRef.current && !assetMenuRef.current.contains(event.target as Node)) {
        setShowAssetMenu(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const loadSpotOpps = async () => {
    setLoading(true);
    try {
      const data = await api.scanSpotOpportunities("");
      setOpportunities(data);
      if (selectedOpp) {
        const refreshed = data.find((o) => o.id === selectedOpp.id);
        if (refreshed) setSelectedOpp(refreshed);
      }
    } catch (err) {
      console.error("Failed to load spot opportunities:", err);
    } finally {
      setLoading(false);
    }
  };

  // Fail closed: an absent descriptor must not arm a stubbed path.
  const executeReady = capability?.executeReady === true;

  useEffect(() => {
    if (capability == null || !capability.scanReady) return;
    loadSpotOpps();
    const interval = setInterval(loadSpotOpps, 3000);
    return () => clearInterval(interval);
  }, [capability]);

  const handleSelectOpp = (opp: SpotOpportunityDto) => {
    setSelectedOpp(opp);
    setBotStatus("idle");
    setBotMessage("");
  };

  const handleDeployBot = async () => {
    if (!selectedOpp) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") {
      setBotStatus("error");
      setBotMessage(`Entry blocked: bot is ${bot.phase.replace("_", "-")} — only exits are permitted`);
      return;
    }
    setBotStatus("verifying");
    setBotMessage("Verifying pre-funded inventory & orderbook depth...");

    try {
      await new Promise((r) => setTimeout(r, 400));
      setBotStatus("dispatching");
      setBotMessage(`Dispatching atomic dual-leg orders to ${selectedOpp.buy_venue} and ${selectedOpp.sell_venue}...`);

      const params: SpotExecutionParams = {
        symbol: selectedOpp.symbol,
        buy_venue: selectedOpp.buy_venue,
        sell_venue: selectedOpp.sell_venue,
        trade_size_usd: tradeSizeUsd,
        max_slippage_bps: maxSlippageBps,
      };

      const result = await api.executeSpotArbitrage(params);
      await onRefreshAll();
      await loadSpotOpps();

      setBotStatus("filled");
      setBotMessage(`✓ Flash fill confirmed! Net Profit: +$${result.realized_profit_usd.toFixed(2)} USD (+${result.spread_bps.toFixed(1)} BPS) in ${selectedOpp.estimated_latency_ms}ms.`);
    } catch (err: unknown) {
      setBotStatus("error");
      setBotMessage(err instanceof Error ? err.message : "Execution failed");
    }
  };

  // Automated Execution Loop when user manually enables the Spot Bot
  useEffect(() => {
    if (!executeReady || !bot.entriesAllowed || botStatus === "verifying" || botStatus === "dispatching" || isPaused) return;

    const now = Date.now();
    if (now - lastAutoSpotExecRef.current < 5000) return; // 5s cooldown

    const bestCandidate = filteredOpportunities.find((o) => o.net_spread_bps >= minNetSpreadBps);
    if (bestCandidate) {
      lastAutoSpotExecRef.current = now;
      setSelectedOpp(bestCandidate);
      handleDeployBot();
    }
  }, [bot.entriesAllowed, botStatus, filteredOpportunities, isPaused, minNetSpreadBps]);

  // Spot executions attribute themselves under the "spot" strategy id —
  // durable, unlike the shared two-leg path's "Spatial" trade-type label.
  const spotLogs = useMemo(
    () =>
      executionLogs.filter((l) => {
        const s = (l.strategy_id ?? "").toLowerCase();
        return s === "spot" || s === "spatial";
      }),
    [executionLogs]
  );

  const bestOpp = filteredOpportunities.length > 0 ? filteredOpportunities[0] : null;
  const totalSpotPnl = spotLogs.reduce((acc, l) => acc + (l.net_profit || 0), 0);

  if (!executeReady) {
    return (
      <div className="flex-1 flex flex-col h-full bg-background overflow-y-auto">
        <div className="flex-1 flex items-center justify-center p-8">
          <EmptyState
            icon={AlertCircle}
            title="Spot Arbitrage Unavailable"
            hint={capability?.reason ?? "The backend spot execution path is not implemented."}
          />
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col h-full bg-background overflow-y-auto p-4 pr-5 space-y-4">
      {/* Strategy Ribbon — dedicated zone: parameters left, configurator + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Status
          </span>
          <BotPhaseBadge phase={bot.phase} isPaused={isPaused} />
          {/* Trade Size */}
          <div
            className="ribbon-pill"
            title="Capital deployed per flash cycle — edit via Configurator"
          >
            <span className="text-muted">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSizeUsd.toLocaleString()}</span>
            <span className="text-muted">USD</span>
          </div>

          {/* Minimum Net Spread required to fire */}
          <div
            className="ribbon-pill"
            title="Net spread required to auto-execute — edit via Configurator"
          >
            <span className="text-muted">Min Net</span>
            <span className="font-bold text-slate-900 tabular-nums">{minNetSpreadBps}</span>
            <span className="text-muted">BPS</span>
          </div>

          {/* Max Slippage */}
          <div
            className="ribbon-pill"
            title="Orderbook tolerance limit — edit via Configurator"
          >
            <span className="text-muted">Max Slippage</span>
            <span className="font-bold text-slate-900 tabular-nums">{maxSlippageBps}</span>
            <span className="text-muted">BPS</span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          <button
            type="button"
            onClick={() => onOpenConfig?.(selectedOpp?.symbol || "BTC-USDC", "spot")}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Spatial strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent-strong" />
            <span>Configurator</span>
          </button>

          <BotLifecycleControls
            phase={bot.phase}
            hasOpenPositions={bot.hasOpenPositions}
            disabled={isPaused || !hasApiConnectivity}
            onStart={bot.start}
            onPause={bot.pause}
            onReduceOnly={bot.reduceOnly}
            onStop={() => void bot.stop()}
          />
          {bot.lastError && (
            <span className="text-xs font-mono text-loss max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
        </div>
      </div>

      {/* Summary Telemetry Metrics Bar */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-muted font-mono">Scanned Discrepancies</div>
          <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">{filteredOpportunities.length} Pairs</div>
          <div className="text-xs text-slate-500 mt-0.5">
            {filteredOpportunities.length > 0 ? "Live Orderbook Depth" : "Zero synthetic pairs"}
          </div>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-muted font-mono">Max Net Spread</div>
          <div className="text-lg font-bold text-profit font-mono tabular-nums mt-0.5">
            {bestOpp ? `+${bestOpp.net_spread_bps.toFixed(1)} BPS` : "—"}
          </div>
          <div className="text-xs text-slate-500 mt-0.5">
            {bestOpp ? `${bestOpp.symbol} (${bestOpp.buy_venue} ➔ ${bestOpp.sell_venue})` : (loading ? "Scanning..." : "Requires configured spot venues")}
          </div>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-muted font-mono">Target Bot Latency</div>
          <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
            {bestOpp ? `~${bestOpp.estimated_latency_ms.toFixed(1)} ms` : "—"}
          </div>
          <div className="text-xs text-slate-500 mt-0.5">
            {filteredOpportunities.length > 0 ? "Concurrent Atomic Dispatch" : "Idle (No active pairs)"}
          </div>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-muted font-mono">Realized Spot Profit</div>
          <div className="text-lg font-bold text-profit font-mono tabular-nums mt-0.5">+${totalSpotPnl.toFixed(2)} USD</div>
          <div className="text-xs text-slate-500 mt-0.5">{spotLogs.length} Bot Fills</div>
        </div>
      </div>

      {/* Main Content Workspace: Full-Width Opportunity Scanner */}
      <div className="flex-1 min-h-0 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
        {/* Filter Bar */}
        <div className="p-3.5 border-b border-surface-border flex items-center justify-between gap-3 bg-surface">
          <h3 className="text-[13px] font-bold font-sans tracking-wider text-slate-900">
            Opportunity Scanner
          </h3>
          <div className="flex items-center gap-3 text-xs font-sans">
            <div className="flex items-center gap-1.5 bg-surface border border-surface-border rounded-lg px-2.5 py-1.5 shadow-2xs hover:border-slate-300 focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 transition-all">
              <Search size={13} className="text-muted shrink-0" />
              <span className="text-muted font-medium text-xs">Search</span>
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="ALL"
                className="bg-transparent w-24 outline-none font-bold text-slate-900 uppercase font-mono text-xs placeholder:text-slate-300"
              />
            </div>

            <div className="flex items-center space-x-2">
              <span className="text-xs font-bold text-slate-700 font-mono">Filter Asset:</span>
              <div className="relative" ref={assetMenuRef}>
                <button
                  type="button"
                  onClick={() => setShowAssetMenu((v) => !v)}
                  className="dropdown-trigger" data-open={showAssetMenu}
                  title="Display filter — shows only opportunities on the selected assets"
                >
                  <span className="font-bold text-slate-900">
                    {assetFilters.length === 0 ? "All" : assetFilters.join("+")}
                  </span>
                  <ChevronDown
                    className={`dropdown-chevron ${showAssetMenu ? "rotate-180" : ""}`}
                  />
                </button>
                {showAssetMenu && (
                  <div className="dropdown-menu right-0 top-full mt-1.5 w-48 z-30">
                    <button
                      type="button"
                      onClick={() => setAssetFilters([])}
                      className={`dropdown-item ${assetFilters.length === 0 ? "dropdown-item-selected" : ""}`}
                    >
                      <span>All</span>
                      {assetFilters.length === 0 && <Check className="dropdown-check" />}
                    </button>
                    <div className="dropdown-divider" />
                    {assets.map((a) => {
                      const on = assetFilters.includes(a);
                      return (
                        <button
                          key={a}
                          type="button"
                          onClick={() =>
                            setAssetFilters((prev) =>
                              prev.includes(a) ? prev.filter((x) => x !== a) : [...prev, a],
                            )
                          }
                          className={`dropdown-item ${on ? "dropdown-item-selected" : ""}`}
                        >
                          <span>{a}</span>
                          {on && <Check className="dropdown-check" />}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
            <div className="text-xs font-mono text-slate-500">
              {filteredOpportunities.length} opportunities detected
            </div>
          </div>
        </div>

        {/* Table */}
        <div className="flex-1 overflow-x-auto overflow-y-auto max-h-[360px]">
          <table className="w-full text-left text-xs font-mono select-none">
            <thead className="sticky top-0 z-10 bg-surface-raised text-xs text-slate-500 tracking-wider border-b border-surface-border">
              <tr>
                <th className="py-2.5 px-4 font-semibold">Pair</th>
                <th className="py-2.5 px-4 font-semibold">Buy Venue (Ask)</th>
                <th className="py-2.5 px-4 font-semibold">Sell Venue (Bid)</th>
                <th className="py-2.5 px-4 font-semibold text-right">Gross BPS</th>
                <th className="py-2.5 px-4 font-semibold text-right">Friction (Fees+Gas)</th>
                <th className="py-2.5 px-4 font-semibold text-right">Net Spread</th>
                <th className="py-2.5 px-4 font-semibold text-right">Est. Profit ($)</th>
                <th className="py-2.5 px-4 font-semibold text-center">Action</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-surface-border">
              {filteredOpportunities.length === 0 ? (
                <tr>
                  <td colSpan={8}>
                    <EmptyState plain title="No Opportunity Detected" />
                  </td>
                </tr>
              ) : (
                filteredOpportunities.map((opp) => {
                  const isSelected = selectedOpp?.id === opp.id;
                  const isProfitable = opp.net_spread_bps > 0;
                  return (
                    <tr
                      key={opp.id}
                      onClick={() => {
                        handleSelectOpp(opp);
                        setShowStageModal(true);
                      }}
                      className={`cursor-pointer transition ${
                        isSelected
                          ? "bg-blue-50/70 border-l-4 border-l-accent"
                          : "hover:bg-surface-raised/50"
                      }`}
                    >
                      <td className="py-3 px-4 font-bold text-slate-900">
                        {opp.symbol}
                      </td>
                      <td className="py-3 px-4">
                        <div className="font-semibold text-slate-800">{opp.buy_venue}</div>
                        <div className="text-xs text-slate-500 font-mono tabular-nums">${opp.buy_price.toLocaleString(undefined, { minimumFractionDigits: 2 })}</div>
                      </td>
                      <td className="py-3 px-4">
                        <div className="font-semibold text-slate-800">{opp.sell_venue}</div>
                        <div className="text-xs text-slate-500 font-mono tabular-nums">${opp.sell_price.toLocaleString(undefined, { minimumFractionDigits: 2 })}</div>
                      </td>
                      <td className="py-3 px-4 text-right text-slate-700 tabular-nums">
                        +{opp.gross_spread_bps.toFixed(1)}
                      </td>
                      <td className="py-3 px-4 text-right text-slate-500 tabular-nums">
                        -{(opp.buy_fee_bps + opp.sell_fee_bps).toFixed(1)} bps (${opp.transfer_cost_usd.toFixed(2)})
                      </td>
                      <td className="py-3 px-4 text-right">
                        <span
                          className={`px-2 py-0.5 rounded-md font-bold tabular-nums ${
                            isProfitable
                              ? "bg-profit-dim text-profit border border-profit-border"
                              : "bg-surface-raised text-slate-600 border border-surface-border"
                          }`}
                        >
                          +{opp.net_spread_bps.toFixed(1)} BPS
                        </span>
                      </td>
                      <td className="py-3 px-4 text-right font-bold text-profit tabular-nums">
                        +${opp.net_profit_usd.toFixed(2)}
                      </td>
                      <td className="py-3 px-4 text-center">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleSelectOpp(opp);
                            setShowStageModal(true);
                          }}
                          className={`px-3 py-1.5 rounded-lg text-xs font-medium transition cursor-pointer ${
                            isSelected
                              ? "btn-primary !py-1.5 !px-3 !text-xs"
                              : "bg-surface-raised border border-surface-border text-slate-700 hover:border-accent hover:text-accent-strong"
                          }`}
                        >
                          Stage / Deploy
                        </button>
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </div>

      {/* Bottom Dock: Current Positions vs Execution History vs Opportunity History vs Log Monitor */}
      <div className="bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden shrink-0">
        <div className="grid grid-cols-4 border-b border-surface-border bg-slate-50/60 select-none pt-1 px-1 gap-1">
          {(
            [
              { id: "position", label: "Open Positions", icon: Wallet },
              { id: "opps", label: "Opportunity History", icon: Radar },
              { id: "history", label: "Execution History", icon: History },
              { id: "logs", label: "Log Monitor", icon: Terminal },
            ] as const
          ).map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              onClick={() => setDockTab(id)}
              className={`w-full flex items-center justify-center space-x-2 py-2 px-3 text-xs font-sans transition select-none group cursor-pointer rounded-t-lg ${
                dockTab === id
                  ? "text-accent-strong font-bold bg-surface border-t border-x border-surface-border -mb-px shadow-2xs"
                  : "text-slate-600 hover:text-slate-900 hover:bg-slate-100/80 font-medium border-t border-x border-transparent"
              }`}
            >
              <Icon
                className={`w-3.5 h-3.5 transition shrink-0 ${
                  dockTab === id ? "text-accent-strong" : "text-muted group-hover:text-slate-600"
                }`}
              />
              <span className="truncate">{label}</span>
            </button>
          ))}
        </div>
        <div className="p-3.5 h-96 overflow-y-auto">
          {dockTab === "position" ? (
            spotPositions.length === 0 ? (
              <EmptyState
                plain
                title="No Open Spatial Positions"
                hint="Spatial dual-leg fills aggregate here per symbol with live PnL. Working positions resolve upon fill or automatic balance rebalance."
              />
            ) : (
              <table className="w-full text-left text-xs font-mono tabular-nums">
                <thead>
                  <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
                    <th className="py-2.5 px-3">Symbol</th>
                    <th className="py-2.5 px-3">Venues</th>
                    <th className="py-2.5 px-3 text-right">Notional</th>
                    <th className="py-2.5 px-3 text-right">Unrealized P&L</th>
                    <th className="py-2.5 px-3 text-right">Status</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border">
                  {spotPositions.map((pos) => {
                    const pnl = pos.unrealizedPnlUsd;
                    return (
                      <tr
                        key={pos.key}
                        onClick={() => setSelectedDetailPos(pos)}
                        className="hover:bg-surface-raised/60 transition cursor-pointer"
                        title="Click to view position details, legs, and PnL trend"
                      >
                        <td className="py-2.5 px-3 whitespace-nowrap font-bold text-slate-900">
                          {pos.symbol}
                        </td>
                        <td className="py-2.5 px-3">
                          {pos.venues.map((v) => `${v.venue}: ${v.netQty}`).join(" / ")}
                        </td>
                        <td className="py-2.5 px-3 text-right text-slate-700">
                          {pos.notionalUsd != null ? `$${pos.notionalUsd.toLocaleString(undefined, { maximumFractionDigits: 0 })}` : "—"}
                        </td>
                        <td className={`py-2.5 px-3 text-right font-bold ${pnl != null && pnl < 0 ? "text-loss" : "text-profit"}`}>
                          {pnl != null ? `${pnl >= 0 ? "+" : "-"}$${Math.abs(pnl).toFixed(2)}` : "—"}
                        </td>
                        <td className="py-2.5 px-3 text-right">
                          <span className="px-2 py-0.5 rounded-full text-[10px] font-bold border border-profit-border bg-profit-dim/40 text-profit">
                            Working
                          </span>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )
          ) : dockTab === "history" ? (
            <TickerTransactionHistory symbol="" logs={spotLogs} matchAllSymbols />
          ) : dockTab === "opps" ? (
            <TradedOpportunitiesTable
              opps={tradedOpps}
              failed={tradedFailed}
              onSelect={setSelectedTradedOpp}
            />
          ) : (
            <TickerLogMonitor
              symbol=""
              namespace="spatial-arbitrage"
              strategyName="Spatial Arbitrage"
              executionLogs={spotLogs}
              matchAllSymbols
              isPaused={isPaused}
            />
          )}
        </div>
      </div>

      {/* Modal: Fast Execution Bot Staging & Deployment */}
      {showStageModal && selectedOpp && (
        <SpotExecutionModal
          opp={selectedOpp}
          tradeSizeUsd={tradeSizeUsd}
          maxSlippageBps={maxSlippageBps}
          botStatus={botStatus}
          botMessage={botMessage}
          onDeploy={handleDeployBot}
          onClose={() => setShowStageModal(false)}
        />
      )}

      {/* Modal: Traded Opportunity Detail */}
      {selectedTradedOpp && (
        <TradedOpportunityDetailModal
          opp={selectedTradedOpp}
          onClose={() => setSelectedTradedOpp(null)}
        />
      )}

      {/* Modal: Position Detail */}
      {selectedDetailPos && (
        <PositionDetailModal
          pos={selectedDetailPos}
          onClose={() => setSelectedDetailPos(null)}
        />
      )}
    </div>
  );
};

interface SpotExecutionModalProps {
  opp: SpotOpportunityDto;
  tradeSizeUsd: number;
  maxSlippageBps: number;
  botStatus: "idle" | "verifying" | "dispatching" | "filled" | "error";
  botMessage: string;
  onDeploy: () => void;
  onClose: () => void;
}

const SpotExecutionModal: React.FC<SpotExecutionModalProps> = ({
  opp,
  tradeSizeUsd,
  maxSlippageBps,
  botStatus,
  botMessage,
  onDeploy,
  onClose,
}) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 bg-slate-900/50 backdrop-blur-sm flex items-center justify-center p-3 sm:p-6 select-none"
      role="dialog"
      aria-modal="true"
      aria-label={`Fast Execution Bot — ${opp.symbol}`}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="bg-surface border border-surface-border rounded-2xl w-[640px] max-w-[96vw] max-h-[90vh] flex flex-col shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between px-6 py-3.5 border-b border-surface-border bg-surface shrink-0">
          <div className="flex items-center space-x-2">
            <Zap className="w-4 h-4 text-accent-strong" />
            <h2 className="text-sm font-bold text-slate-900 font-mono tracking-wider">
              Fast Execution Bot — {opp.symbol}
            </h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded-lg text-slate-400 hover:text-slate-600 hover:bg-slate-100 transition cursor-pointer"
            aria-label="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="p-6 space-y-4 overflow-y-auto">
          {/* Selected Opportunity Card */}
          <div className="p-3.5 bg-accent-dim/40 rounded-xl border border-accent-line/60 space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-xs font-bold text-slate-900 font-mono">
                {opp.symbol} Spot Route
              </span>
              <span className="text-xs font-bold text-profit font-mono tabular-nums">
                +{opp.net_spread_bps.toFixed(1)} BPS Net
              </span>
            </div>

            <div className="text-xs font-mono text-slate-700 flex items-center justify-between">
              <span>BUY: {opp.buy_venue}</span>
              <span className="font-semibold tabular-nums">${opp.buy_price.toLocaleString()}</span>
            </div>
            <div className="text-xs font-mono text-slate-700 flex items-center justify-between">
              <span>SELL: {opp.sell_venue}</span>
              <span className="font-semibold tabular-nums">${opp.sell_price.toLocaleString()}</span>
            </div>
          </div>

          {/* Trade Size & Slippage */}
          <div className="grid grid-cols-2 gap-3 text-xs font-mono">
            <div className="p-3 bg-surface-raised rounded-lg border border-surface-border">
              <span className="text-muted block mb-1">Trade Size</span>
              <span className="font-bold text-slate-900 text-sm tabular-nums">${tradeSizeUsd.toLocaleString()} USD</span>
            </div>
            <div className="p-3 bg-surface-raised rounded-lg border border-surface-border">
              <span className="text-muted block mb-1">Max Allowed Slippage</span>
              <span className="font-bold text-slate-900 text-sm tabular-nums">{maxSlippageBps.toFixed(1)} BPS</span>
            </div>
          </div>

          {/* Pre-flight Checks */}
          <div className="p-3 bg-surface-raised rounded-lg border border-surface-border text-xs font-mono space-y-2 text-slate-600">
            <div className="flex items-center justify-between">
              <span className="flex items-center space-x-1.5 text-slate-700">
                <AlertCircle className="w-3.5 h-3.5 text-muted" />
                <span>Buy Venue Inventory</span>
              </span>
              <span className="font-semibold text-muted">Unverified</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="flex items-center space-x-1.5 text-slate-700">
                <AlertCircle className="w-3.5 h-3.5 text-muted" />
                <span>Sell Venue Inventory</span>
              </span>
              <span className="font-semibold text-muted">Unverified</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="flex items-center space-x-1.5 text-slate-700">
                <Clock className="w-3.5 h-3.5 text-accent-strong" />
                <span>Route Latency</span>
              </span>
              <span className="font-semibold text-slate-900 tabular-nums">~{opp.estimated_latency_ms} ms</span>
            </div>
            <div className="flex items-center justify-between pt-1 border-t border-surface-border font-bold">
              <span className="text-slate-800">Net Expected Profit:</span>
              <span className="text-profit tabular-nums">
                +${((tradeSizeUsd * opp.net_spread_bps) / 10000).toFixed(2)} USD
              </span>
            </div>
          </div>

          {/* Capacity curve */}
          <div>
            <div className="text-[11px] font-sans font-bold uppercase tracking-wider text-muted mb-1.5 flex items-center">
              Capacity curve
              <InfoTooltip text="Profit vs position size walked through both leg orderbooks. Empty until the spot scan emits book depth." position="top" />
            </div>
            {(opp.depth_curve?.length ?? 0) >= 2 ? (
              <CapacityChart points={opp.depth_curve ?? []} />
            ) : (
              <span className="text-muted text-xs font-mono">
                no depth data — spot scan is stubbed upstream
              </span>
            )}
          </div>

          {/* Bot Progress & Action */}
          {botMessage && (
            <div
              className={`p-3 rounded-lg border text-xs font-mono ${
                botStatus === "filled"
                  ? "bg-profit-dim text-profit border-profit-border"
                  : botStatus === "error"
                  ? "bg-loss-dim text-loss border-loss-border"
                  : "bg-accent-dim text-accent-strong border-accent-line animate-pulse"
              }`}
            >
              {botMessage}
            </div>
          )}
        </div>

        <div className="px-6 py-3.5 border-t border-surface-border bg-surface flex items-center justify-end gap-2 shrink-0">
          <button
            type="button"
            onClick={onClose}
            className="btn-secondary !text-xs !py-2 !px-3 font-sans cursor-pointer"
          >
            Close
          </button>
          <ExecuteButton
            onClick={onDeploy}
            executing={botStatus === "verifying" || botStatus === "dispatching"}
            title="dispatch atomic dual-leg orders"
            className="!py-2 !px-4 text-xs cursor-pointer"
          />
        </div>
      </div>
    </div>
  );
};
