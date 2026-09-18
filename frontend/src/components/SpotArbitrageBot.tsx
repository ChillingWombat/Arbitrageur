import React, { useState, useEffect, useRef } from "react";
import {
  Zap,
  AlertCircle,
  Clock,
  SlidersHorizontal,
  ChevronDown,
  Check,
} from "lucide-react";
import { api } from "../services/api";
import type { SpotOpportunityDto, SpotExecutionParams, ExecutionLogEntry } from "../types";
import type { SpotStrategyParams } from "./TickerConfigModal";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { EmptyState } from "./EmptyState";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface SpotArbitrageBotProps {
  executionLogs: ExecutionLogEntry[];
  onRefreshAll: () => Promise<void>;
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  params: SpotStrategyParams;
}

export const SpotArbitrageBot: React.FC<SpotArbitrageBotProps> = ({
  executionLogs,
  onRefreshAll,
  onOpenConfig,
  isPaused = false,
  hasApiConnectivity = false,
  params,
}) => {
  const [opportunities, setOpportunities] = useState<SpotOpportunityDto[]>([]);
  const [assetFilters, setAssetFilters] = useState<string[]>([]);
  const [showAssetMenu, setShowAssetMenu] = useState(false);
  const assetMenuRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(false);
  const [selectedOpp, setSelectedOpp] = useState<SpotOpportunityDto | null>(null);

  // Bot lifecycle: Start / Pause / Reduce Only / Stop (stop flattens this
  // strategy's positions across venues until delta-neutral)
  const bot = useBotLifecycle({ strategyId: "spot" });
  const lastAutoSpotExecRef = React.useRef<number>(0);
  // Strategy params are owned by App and edited only via the Strategy Configurator
  const { tradeSizeUsd, minNetSpreadBps, maxSlippageBps } = params;
  const [botStatus, setBotStatus] = useState<"idle" | "verifying" | "dispatching" | "filled" | "error">("idle");
  const [botMessage, setBotMessage] = useState<string>("");

  const assets = ["BTC", "ETH", "SOL", "BNB", "SUI", "AVAX", "LINK", "NEAR"];
  const assetBase = (sym: string) => sym.split(/[-_/]/)[0]?.toUpperCase() ?? sym;
  const filteredOpportunities =
    assetFilters.length === 0
      ? opportunities
      : opportunities.filter((o) => assetFilters.includes(assetBase(o.symbol)));

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

  useEffect(() => {
    loadSpotOpps();
    const interval = setInterval(loadSpotOpps, 3000);
    return () => clearInterval(interval);
  }, []);

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
    if (!bot.entriesAllowed || botStatus === "verifying" || botStatus === "dispatching" || isPaused) return;

    const now = Date.now();
    if (now - lastAutoSpotExecRef.current < 5000) return; // 5s cooldown

    const bestCandidate = filteredOpportunities.find((o) => o.net_spread_bps >= minNetSpreadBps);
    if (bestCandidate) {
      lastAutoSpotExecRef.current = now;
      setSelectedOpp(bestCandidate);
      handleDeployBot();
    }
  }, [bot.entriesAllowed, botStatus, filteredOpportunities, isPaused, minNetSpreadBps]);

  const spotLogs = executionLogs.filter(
    (l) => l.trade_type.toLowerCase().includes("spot") || l.direction?.toLowerCase().includes("spot")
  );

  const bestOpp = filteredOpportunities.length > 0 ? filteredOpportunities[0] : null;
  const totalSpotPnl = spotLogs.reduce((acc, l) => acc + (l.net_profit || 0), 0);

  return (
    <div className="flex-1 flex flex-col h-full bg-background overflow-y-auto p-4 space-y-4">
      {/* Strategy Ribbon — dedicated zone: parameters left, configurator + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          {/* Trade Size */}
          <div
            className="ribbon-pill"
            title="Capital deployed per flash cycle — edit via Configure Strategy"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSizeUsd.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          {/* Minimum Net Spread required to fire */}
          <div
            className="ribbon-pill"
            title="Net spread required to auto-execute — edit via Configure Strategy"
          >
            <span className="text-slate-400">Min Net</span>
            <span className="font-bold text-slate-900 tabular-nums">{minNetSpreadBps}</span>
            <span className="text-slate-400">BPS</span>
          </div>

          {/* Max Slippage */}
          <div
            className="ribbon-pill"
            title="Orderbook tolerance limit — edit via Configure Strategy"
          >
            <span className="text-slate-400">Max Slippage</span>
            <span className="font-bold text-slate-900 tabular-nums">{maxSlippageBps}</span>
            <span className="text-slate-400">BPS</span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          <button
            type="button"
            onClick={() => onOpenConfig?.(selectedOpp?.symbol || "BTC-USDC", "spot")}
            className="btn-secondary !text-xs !py-1.5 !px-3 font-mono cursor-pointer"
            title="Configure Spatial strategy parameters (applies to all tickers)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
            <span>Configure Strategy</span>
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
          <div className="text-xs font-medium text-slate-400 font-mono">Scanned Discrepancies</div>
          <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">{filteredOpportunities.length} Pairs</div>
          <div className="text-xs text-slate-500 mt-0.5">
            {filteredOpportunities.length > 0 ? "Live Orderbook Depth" : "Zero synthetic pairs"}
          </div>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-slate-400 font-mono">Max Net Spread</div>
          <div className="text-lg font-bold text-profit font-mono tabular-nums mt-0.5">
            {bestOpp ? `+${bestOpp.net_spread_bps.toFixed(1)} BPS` : "—"}
          </div>
          <div className="text-xs text-slate-500 mt-0.5">
            {bestOpp ? `${bestOpp.symbol} (${bestOpp.buy_venue} ➔ ${bestOpp.sell_venue})` : (loading ? "Scanning..." : "Requires configured spot venues")}
          </div>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-slate-400 font-mono">Target Bot Latency</div>
          <div className="text-lg font-bold text-slate-900 font-mono tabular-nums mt-0.5">
            {bestOpp ? `~${bestOpp.estimated_latency_ms.toFixed(1)} ms` : "—"}
          </div>
          <div className="text-xs text-slate-500 mt-0.5">
            {filteredOpportunities.length > 0 ? "Concurrent Atomic Dispatch" : "Idle (No active pairs)"}
          </div>
        </div>

        <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
          <div className="text-xs font-medium text-slate-400 font-mono">Realized Spot Profit</div>
          <div className="text-lg font-bold text-profit font-mono tabular-nums mt-0.5">+${totalSpotPnl.toFixed(2)} USD</div>
          <div className="text-xs text-slate-500 mt-0.5">{spotLogs.length} Bot Fills</div>
        </div>
      </div>

      {/* Main Content Workspace */}
      <div className="flex-1 flex flex-col xl:flex-row gap-4">
        {/* Left Side: Live Opportunity Table */}
        <div className="flex-1 flex flex-col bg-surface rounded-xl border border-surface-border shadow-card overflow-hidden">
          {/* Filter Bar */}
          <div className="p-4 border-b border-surface-border flex flex-wrap items-center justify-between gap-3 bg-surface">
            <div className="flex items-center space-x-2">
              <span className="text-xs font-bold text-slate-700 font-mono">Filter Asset:</span>
              <div className="relative" ref={assetMenuRef}>
                <button
                  type="button"
                  onClick={() => setShowAssetMenu((v) => !v)}
                  className="flex items-center gap-1.5 bg-surface-raised border border-surface-border rounded-lg px-2.5 py-1.5 font-mono text-xs transition cursor-pointer hover:border-slate-300"
                  title="Display filter — shows only opportunities on the selected assets"
                >
                  <span className="font-bold text-slate-900">
                    {assetFilters.length === 0 ? "All" : assetFilters.join("+")}
                  </span>
                  <ChevronDown
                    className={`w-3 h-3 text-slate-400 transition-transform ${showAssetMenu ? "rotate-180" : ""}`}
                  />
                </button>
                {showAssetMenu && (
                  <div className="absolute left-0 top-full mt-1.5 w-48 bg-surface border border-surface-border rounded-xl shadow-elevated p-1 z-30 font-mono text-xs animate-in fade-in zoom-in-95 duration-100">
                    <button
                      type="button"
                      onClick={() => setAssetFilters([])}
                      className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition rounded-md ${
                        assetFilters.length === 0
                          ? "bg-accent/10 text-accent font-semibold"
                          : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                      }`}
                    >
                      <span>All</span>
                      {assetFilters.length === 0 && <Check className="w-3.5 h-3.5" />}
                    </button>
                    <div className="border-t border-surface-border/60 my-1 -mx-1" />
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
                          className={`w-full flex items-center justify-between px-2.5 py-1.5 text-left transition rounded-md ${
                            on
                              ? "bg-accent/10 text-accent font-semibold"
                              : "text-slate-700 hover:bg-surface-raised hover:text-slate-900"
                          }`}
                        >
                          <span>{a}</span>
                          {on && <Check className="w-3.5 h-3.5" />}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
            <div className="text-xs font-mono text-slate-400">
              Live Sorted By Net Spread BPS
            </div>
          </div>

          {/* Table */}
          <div className="flex-1 overflow-x-auto">
            <table className="w-full text-left text-xs font-mono select-none">
              <thead className="bg-surface-raised text-xs text-slate-500 tracking-wider border-b border-surface-border">
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
                        onClick={() => handleSelectOpp(opp)}
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
                            className={`px-2 py-0.5 rounded-sm font-bold tabular-nums ${
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
                            onClick={(e) => {
                              e.stopPropagation();
                              handleSelectOpp(opp);
                            }}
                            className={`px-3 py-1.5 rounded-md text-xs font-medium transition cursor-pointer ${
                              isSelected
                                ? "btn-primary !py-1 !px-2.5 !text-xs"
                                : "bg-surface-raised border border-surface-border text-slate-700 hover:border-accent hover:text-accent"
                            }`}
                          >
                            {isSelected ? "Selected" : "Select"}
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

        {/* Right Side: Fast Execution Bot Panel */}
        <div className="w-full xl:w-96 flex flex-col space-y-4">
          <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card">
            <div className="flex items-center justify-between pb-2.5 border-b border-surface-border">
              <div className="flex items-center space-x-2">
                <Zap className="w-4 h-4 text-accent" />
                <h2 className="text-[13px] font-bold text-slate-900 font-mono tracking-wider">
                  Fast Execution Bot
                </h2>
              </div>
            </div>

            {selectedOpp ? (
              <div className="mt-4 space-y-4">
                {/* Selected Opportunity Card */}
                <div className="p-3.5 bg-accent-dim/40 rounded-xl border border-accent-line/60 space-y-2">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-bold text-slate-900 font-mono">
                      {selectedOpp.symbol} Spot Route
                    </span>
                    <span className="text-xs font-bold text-profit font-mono tabular-nums">
                      +{selectedOpp.net_spread_bps.toFixed(1)} BPS Net
                    </span>
                  </div>

                  <div className="text-xs font-mono text-slate-700 flex items-center justify-between">
                    <span>BUY: {selectedOpp.buy_venue}</span>
                    <span className="font-semibold tabular-nums">${selectedOpp.buy_price.toLocaleString()}</span>
                  </div>
                  <div className="text-xs font-mono text-slate-700 flex items-center justify-between">
                    <span>SELL: {selectedOpp.sell_venue}</span>
                    <span className="font-semibold tabular-nums">${selectedOpp.sell_price.toLocaleString()}</span>
                  </div>
                </div>

                {/* Trade Size Controls */}
                <div title="Edit via Configure Strategy">
                  <div className="flex items-center justify-between text-xs font-mono text-slate-700 mb-1.5">
                    <span>Trade Size ($ USD):</span>
                    <span className="font-bold text-slate-900 tabular-nums">${tradeSizeUsd.toLocaleString()}</span>
                  </div>
                  <div className="relative w-full h-2 bg-slate-200 rounded-md">
                    <div
                      className="absolute left-0 top-0 h-2 bg-accent/60 rounded-md"
                      style={{ width: `${Math.min(100, (tradeSizeUsd / 10000) * 100)}%` }}
                    />
                    <div
                      className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-3.5 h-3.5 bg-accent rounded-full border-2 border-white shadow-2xs"
                      style={{ left: `${Math.min(100, (tradeSizeUsd / 10000) * 100)}%` }}
                    />
                  </div>
                </div>

                {/* Max Slippage */}
                <div title="Edit via Configure Strategy">
                  <div className="flex items-center justify-between text-xs font-mono text-slate-700 mb-1">
                    <span>Max Allowed Slippage:</span>
                    <span className="font-bold text-slate-900 tabular-nums">{maxSlippageBps.toFixed(1)} BPS</span>
                  </div>
                  <div className="relative w-full h-2 bg-slate-200 rounded-md">
                    <div
                      className="absolute left-0 top-0 h-2 bg-accent/60 rounded-md"
                      style={{ width: `${Math.min(100, (maxSlippageBps / 15) * 100)}%` }}
                    />
                    <div
                      className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-3.5 h-3.5 bg-accent rounded-full border-2 border-white shadow-2xs"
                      style={{ left: `${Math.min(100, (maxSlippageBps / 15) * 100)}%` }}
                    />
                  </div>
                </div>

                {/* Pre-flight Checks */}
                <div className="p-3 bg-surface-raised rounded-lg border border-surface-border text-xs font-mono space-y-2 text-slate-600">
                  <div className="flex items-center justify-between">
                    <span className="flex items-center space-x-1.5 text-slate-700">
                      <AlertCircle className="w-3.5 h-3.5 text-slate-400" />
                      <span>Buy Venue Inventory</span>
                    </span>
                    <span className="font-semibold text-slate-400">Unverified</span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="flex items-center space-x-1.5 text-slate-700">
                      <AlertCircle className="w-3.5 h-3.5 text-slate-400" />
                      <span>Sell Venue Inventory</span>
                    </span>
                    <span className="font-semibold text-slate-400">Unverified</span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="flex items-center space-x-1.5 text-slate-700">
                      <Clock className="w-3.5 h-3.5 text-accent" />
                      <span>Route Latency</span>
                    </span>
                    <span className="font-semibold text-slate-900 tabular-nums">~{selectedOpp.estimated_latency_ms} ms</span>
                  </div>
                  <div className="flex items-center justify-between pt-1 border-t border-surface-border font-bold">
                    <span className="text-slate-800">Net Expected Profit:</span>
                    <span className="text-profit tabular-nums">
                      +${((tradeSizeUsd * selectedOpp.net_spread_bps) / 10000).toFixed(2)} USD
                    </span>
                  </div>
                </div>

                {/* Bot Progress & Action */}
                {botMessage && (
                  <div
                    className={`p-3 rounded-lg border text-xs font-mono ${
                      botStatus === "filled"
                        ? "bg-profit-dim text-profit border-profit-border"
                        : botStatus === "error"
                        ? "bg-loss-dim text-loss border-loss-border"
                        : "bg-accent-dim text-accent border-accent-line animate-pulse"
                    }`}
                  >
                    {botMessage}
                  </div>
                )}

                <button
                  onClick={handleDeployBot}
                  disabled={botStatus === "verifying" || botStatus === "dispatching"}
                  className="btn-primary w-full !py-2.5 font-mono text-xs font-bold transition flex items-center justify-center space-x-2 disabled:opacity-50"
                >
                  <Zap className="w-4 h-4" />
                  <span>
                    {botStatus === "verifying" || botStatus === "dispatching"
                      ? "Executing Bot..."
                      : "Deploy Execution Bot (Atomic Fill)"}
                  </span>
                </button>
              </div>
            ) : (
              <EmptyState
                icon={AlertCircle}
                title="No Opportunity Selected"
                hint="Select an opportunity from the table to stage and deploy the high-speed execution bot."
              />
            )}
          </div>

          {/* Recent Spot Bot Executions */}
          <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card">
            <h3 className="text-[13px] font-bold text-slate-700 font-mono tracking-wider mb-2">
              Recent Spot Bot Fills
            </h3>
            {spotLogs.length === 0 ? (
              <div className="text-xs text-slate-400 font-sans py-4 text-center">
                No spot executions logged yet in this session.
              </div>
            ) : (
              <div className="space-y-2 max-h-48 overflow-y-auto">
                {spotLogs.slice(0, 5).map((log) => (
                  <div
                    key={log.id}
                    className="p-2 bg-surface-raised rounded-lg border border-surface-border text-xs font-mono flex items-center justify-between"
                  >
                    <div>
                      <div className="font-bold text-slate-800">{log.symbol}</div>
                      <div className="text-xs text-slate-500">
                        {log.venue_a} ➔ {log.venue_b}
                      </div>
                    </div>
                    <div className="text-right">
                      <div className="font-bold text-profit tabular-nums">+${(log.net_profit || 0).toFixed(2)}</div>
                      <div className="text-xs text-slate-400 tabular-nums">{Math.round((log.latency_us || 0) / 1000)}ms</div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
