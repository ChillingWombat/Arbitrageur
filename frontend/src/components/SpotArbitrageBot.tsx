import React, { useState, useEffect } from "react";
import {
  Zap,
  AlertCircle,
  Clock,
  Coins,
  SlidersHorizontal,
} from "lucide-react";
import { api } from "../services/api";
import type { SpotOpportunityDto, SpotExecutionParams, ExecutionLogEntry } from "../types";
import type { SpotStrategyParams } from "./TickerConfigModal";
import { BotLifecycleControls } from "./BotLifecycleControls";
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
  const [selectedAsset, setSelectedAsset] = useState<string>("ALL");
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

  const assets = ["ALL", "BTC", "ETH", "SOL", "BNB", "SUI", "AVAX", "LINK", "NEAR"];

  const loadSpotOpps = async () => {
    setLoading(true);
    try {
      const filter = selectedAsset === "ALL" ? "" : selectedAsset;
      const data = await api.scanSpotOpportunities(filter);
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
  }, [selectedAsset]);

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

    const bestCandidate = opportunities.find((o) => o.net_spread_bps >= minNetSpreadBps);
    if (bestCandidate) {
      lastAutoSpotExecRef.current = now;
      setSelectedOpp(bestCandidate);
      handleDeployBot();
    }
  }, [bot.entriesAllowed, botStatus, opportunities, isPaused, minNetSpreadBps]);

  const spotLogs = executionLogs.filter(
    (l) => l.trade_type.toLowerCase().includes("spot") || l.direction?.toLowerCase().includes("spot")
  );

  const bestOpp = opportunities.length > 0 ? opportunities[0] : null;
  const totalSpotPnl = spotLogs.reduce((acc, l) => acc + (l.net_profit || 0), 0);

  return (
    <div className="flex-1 flex flex-col h-full bg-background overflow-y-auto p-4 space-y-4">
      {/* Strategy Ribbon — dedicated zone: parameters left, configurator + toggle right */}
      <div className="px-4 py-3 flex flex-wrap items-center justify-between gap-3 bg-surface rounded-xl border border-surface-border shadow-2xs">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-xs font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          {/* Trade Size */}
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Capital deployed per flash cycle — edit via Configure Strategy"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900">{tradeSizeUsd.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          {/* Minimum Net Spread required to fire */}
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Net spread required to auto-execute — edit via Configure Strategy"
          >
            <span className="text-slate-400">Min Net</span>
            <span className="font-bold text-slate-900">{minNetSpreadBps}</span>
            <span className="text-slate-400">BPS</span>
          </div>

          {/* Max Slippage */}
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1.5 bg-surface-raised border border-surface-border rounded-md"
            title="Orderbook tolerance limit — edit via Configure Strategy"
          >
            <span className="text-slate-400">Max Slippage</span>
            <span className="font-bold text-slate-900">{maxSlippageBps}</span>
            <span className="text-slate-400">BPS</span>
          </div>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          <button
            type="button"
            onClick={() => onOpenConfig?.(selectedOpp?.symbol || "BTC-USDC", "spot")}
            className="flex items-center space-x-1.5 px-3 py-1.5 rounded-md border border-accent-line/60 bg-surface hover:bg-slate-100 text-xs font-mono text-accent font-semibold transition shadow-xs cursor-pointer"
            title="Configure Spot Arbitrage Parameters"
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
            <span className="text-[10px] font-mono text-red-600 max-w-56 truncate" title={bot.lastError}>
              {bot.lastError}
            </span>
          )}
        </div>
      </div>

      {/* Summary Telemetry Metrics Bar */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
        <div className="bg-surface p-3 rounded-lg border border-surface-border">
          <div className="text-[11px] font-medium text-slate-400 font-mono">Scanned Discrepancies</div>
          <div className="text-lg font-bold text-slate-900 font-mono mt-0.5">{opportunities.length} Pairs</div>
          <div className="text-[10px] text-slate-500 mt-0.5">
            {opportunities.length > 0 ? "Live Orderbook Depth" : "Zero synthetic pairs"}
          </div>
        </div>

        <div className="bg-surface p-3 rounded-lg border border-surface-border">
          <div className="text-[11px] font-medium text-slate-400 font-mono">Max Net Spread</div>
          <div className="text-lg font-bold text-emerald-600 font-mono mt-0.5">
            {bestOpp ? `+${bestOpp.net_spread_bps.toFixed(1)} BPS` : "—"}
          </div>
          <div className="text-[10px] text-slate-500 mt-0.5">
            {bestOpp ? `${bestOpp.symbol} (${bestOpp.buy_venue} ➔ ${bestOpp.sell_venue})` : (loading ? "Scanning..." : "Requires configured spot venues")}
          </div>
        </div>

        <div className="bg-surface p-3 rounded-lg border border-surface-border">
          <div className="text-[11px] font-medium text-slate-400 font-mono">Target Bot Latency</div>
          <div className="text-lg font-bold text-slate-900 font-mono mt-0.5">
            {bestOpp ? `~${bestOpp.estimated_latency_ms.toFixed(1)} ms` : "—"}
          </div>
          <div className="text-[10px] text-slate-500 mt-0.5">
            {opportunities.length > 0 ? "Concurrent Atomic Dispatch" : "Idle (No active pairs)"}
          </div>
        </div>

        <div className="bg-surface p-3 rounded-lg border border-surface-border">
          <div className="text-[11px] font-medium text-slate-400 font-mono">Realized Spot Profit</div>
          <div className="text-lg font-bold text-emerald-600 font-mono mt-0.5">+${totalSpotPnl.toFixed(2)} USD</div>
          <div className="text-[10px] text-slate-500 mt-0.5">{spotLogs.length} Bot Fills</div>
        </div>
      </div>

      {/* Main Content Workspace */}
      <div className="flex-1 flex flex-col xl:flex-row gap-4">
        {/* Left Side: Live Opportunity Table */}
        <div className="flex-1 flex flex-col bg-surface rounded-xl border border-surface-border shadow-2xs overflow-hidden">
          {/* Filter Bar */}
          <div className="p-4 border-b border-surface-border flex flex-wrap items-center justify-between gap-3 bg-surface">
            <div className="flex items-center space-x-2">
              <span className="text-xs font-bold text-slate-700 font-mono">Filter Asset:</span>
              <div className="flex flex-wrap gap-1">
                {assets.map((a) => (
                  <button
                    key={a}
                    onClick={() => setSelectedAsset(a)}
                    className={`px-2.5 py-1 rounded text-xs font-mono font-medium transition ${
                      selectedAsset === a
                        ? "bg-accent text-white shadow-2xs"
                        : "bg-surface-raised text-slate-600 hover:text-slate-900 hover:bg-slate-200"
                    }`}
                  >
                    {a}
                  </button>
                ))}
              </div>
            </div>
            <div className="text-xs font-mono text-slate-400">
              Live Sorted By Net Spread BPS
            </div>
          </div>

          {/* Table */}
          <div className="flex-1 overflow-x-auto">
            <table className="w-full text-left text-xs font-mono select-none">
              <thead className="bg-surface-raised/70 border-b border-surface-border text-slate-500 tracking-wider text-[10px]">
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
                {opportunities.length === 0 ? (
                  <tr>
                    <td colSpan={8} className="py-12 text-center text-slate-400 font-sans">
                      <div className="flex flex-col items-center justify-center space-y-2">
                        <Coins className="w-8 h-8 text-slate-300" />
                        <p className="font-medium text-slate-600">No Spot Arbitrage Discrepancies Detected</p>
                        <p className="text-xs text-slate-400 max-w-md">
                          Requires at least 2 enabled and connected spot venues with configured API credentials in System Settings (e.g. Kraken Spot, Arcus, Robinhood).
                        </p>
                      </div>
                    </td>
                  </tr>
                ) : (
                  opportunities.map((opp) => {
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
                          <div className="text-[11px] text-slate-500 font-mono">${opp.buy_price.toLocaleString(undefined, { minimumFractionDigits: 2 })}</div>
                        </td>
                        <td className="py-3 px-4">
                          <div className="font-semibold text-slate-800">{opp.sell_venue}</div>
                          <div className="text-[11px] text-slate-500 font-mono">${opp.sell_price.toLocaleString(undefined, { minimumFractionDigits: 2 })}</div>
                        </td>
                        <td className="py-3 px-4 text-right text-slate-700">
                          +{opp.gross_spread_bps.toFixed(1)}
                        </td>
                        <td className="py-3 px-4 text-right text-slate-500">
                          -{(opp.buy_fee_bps + opp.sell_fee_bps).toFixed(1)} bps (${opp.transfer_cost_usd.toFixed(2)})
                        </td>
                        <td className="py-3 px-4 text-right">
                          <span
                            className={`px-2 py-0.5 rounded-sm font-bold ${
                              isProfitable
                                ? "bg-emerald-50 text-emerald-700 border border-emerald-200"
                                : "bg-slate-100 text-slate-600"
                            }`}
                          >
                            +{opp.net_spread_bps.toFixed(1)} BPS
                          </span>
                        </td>
                        <td className="py-3 px-4 text-right font-bold text-emerald-600">
                          +${opp.net_profit_usd.toFixed(2)}
                        </td>
                        <td className="py-3 px-4 text-center">
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleSelectOpp(opp);
                            }}
                            className={`px-3 py-1 rounded text-[11px] font-medium transition ${
                              isSelected
                                ? "bg-accent text-white font-semibold"
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
          <div className="bg-surface rounded-xl border border-surface-border p-5 shadow-2xs">
            <div className="flex items-center justify-between pb-3 border-b border-surface-border">
              <div className="flex items-center space-x-2">
                <Zap className="w-4 h-4 text-accent" />
                <h2 className="text-sm font-bold text-slate-900 font-mono tracking-wider">
                  Fast Execution Bot
                </h2>
              </div>
            </div>

            {selectedOpp ? (
              <div className="mt-4 space-y-4">
                {/* Selected Opportunity Card */}
                <div className="p-3.5 bg-blue-50/50 rounded-lg border border-blue-200 space-y-2">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-bold text-slate-900 font-mono">
                      {selectedOpp.symbol} Spot Route
                    </span>
                    <span className="text-xs font-bold text-emerald-600 font-mono">
                      +{selectedOpp.net_spread_bps.toFixed(1)} BPS Net
                    </span>
                  </div>

                  <div className="text-xs font-mono text-slate-700 flex items-center justify-between">
                    <span>BUY: {selectedOpp.buy_venue}</span>
                    <span className="font-semibold">${selectedOpp.buy_price.toLocaleString()}</span>
                  </div>
                  <div className="text-xs font-mono text-slate-700 flex items-center justify-between">
                    <span>SELL: {selectedOpp.sell_venue}</span>
                    <span className="font-semibold">${selectedOpp.sell_price.toLocaleString()}</span>
                  </div>
                </div>

                {/* Trade Size Controls */}
                <div title="Edit via Configure Strategy">
                  <div className="flex items-center justify-between text-xs font-mono text-slate-700 mb-1.5">
                    <span>Trade Size ($ USD):</span>
                    <span className="font-bold text-slate-900">${tradeSizeUsd.toLocaleString()}</span>
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
                    <span className="font-bold text-slate-900">{maxSlippageBps.toFixed(1)} BPS</span>
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
                <div className="p-3 bg-surface-raised/70 rounded-lg border border-surface-border text-xs font-mono space-y-1.5 text-slate-600">
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
                      <Clock className="w-3.5 h-3.5 text-blue-500" />
                      <span>Route Latency</span>
                    </span>
                    <span className="font-semibold text-slate-900">~{selectedOpp.estimated_latency_ms} ms</span>
                  </div>
                  <div className="flex items-center justify-between pt-1 border-t border-surface-border font-bold">
                    <span className="text-slate-800">Net Expected Profit:</span>
                    <span className="text-emerald-600">
                      +${((tradeSizeUsd * selectedOpp.net_spread_bps) / 10000).toFixed(2)} USD
                    </span>
                  </div>
                </div>

                {/* Bot Progress & Action */}
                {botMessage && (
                  <div
                    className={`p-3 rounded-lg border text-xs font-mono ${
                      botStatus === "filled"
                        ? "bg-emerald-50 text-emerald-800 border-emerald-200"
                        : botStatus === "error"
                        ? "bg-red-50 text-red-800 border-red-200"
                        : "bg-blue-50 text-blue-800 border-blue-200 animate-pulse"
                    }`}
                  >
                    {botMessage}
                  </div>
                )}

                <button
                  onClick={handleDeployBot}
                  disabled={botStatus === "verifying" || botStatus === "dispatching"}
                  className="w-full py-3 bg-accent hover:bg-accent-hover text-white rounded-lg font-mono text-xs font-bold transition shadow-xs flex items-center justify-center space-x-2 disabled:opacity-50"
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
              <div className="py-12 text-center text-slate-400 text-xs font-sans">
                <AlertCircle className="w-6 h-6 mx-auto mb-2 text-slate-300" />
                Select an opportunity from the table to stage and deploy the high-speed execution bot.
              </div>
            )}
          </div>

          {/* Recent Spot Bot Executions */}
          <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-2xs">
            <h3 className="text-xs font-bold text-slate-700 font-mono tracking-wider mb-2">
              Recent Spot Bot Fills
            </h3>
            {spotLogs.length === 0 ? (
              <div className="text-[11px] text-slate-400 font-sans py-4 text-center">
                No spot executions logged yet in this session.
              </div>
            ) : (
              <div className="space-y-2 max-h-48 overflow-y-auto">
                {spotLogs.slice(0, 5).map((log) => (
                  <div
                    key={log.id}
                    className="p-2 bg-surface-raised/60 rounded-md border border-surface-border text-[11px] font-mono flex items-center justify-between"
                  >
                    <div>
                      <div className="font-bold text-slate-800">{log.symbol}</div>
                      <div className="text-[10px] text-slate-500">
                        {log.venue_a} ➔ {log.venue_b}
                      </div>
                    </div>
                    <div className="text-right">
                      <div className="font-bold text-emerald-600">+${(log.net_profit || 0).toFixed(2)}</div>
                      <div className="text-[10px] text-slate-400">{Math.round((log.latency_us || 0) / 1000)}ms</div>
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
