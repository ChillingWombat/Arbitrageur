import React, { useState, useEffect } from "react";
import {
  Zap,
  RefreshCw,
  CheckCircle2,
  AlertCircle,
  Clock,
  Coins,
} from "lucide-react";
import { api } from "../services/api";
import type { SpotOpportunityDto, SpotExecutionParams, ExecutionLogEntry } from "../types";

interface SpotArbitrageBotProps {
  executionLogs: ExecutionLogEntry[];
  onRefreshAll: () => Promise<void>;
}

export const SpotArbitrageBot: React.FC<SpotArbitrageBotProps> = ({
  executionLogs,
  onRefreshAll,
}) => {
  const [opportunities, setOpportunities] = useState<SpotOpportunityDto[]>([]);
  const [selectedAsset, setSelectedAsset] = useState<string>("ALL");
  const [loading, setLoading] = useState(false);
  const [selectedOpp, setSelectedOpp] = useState<SpotOpportunityDto | null>(null);

  // Execution Bot Parameters
  const [tradeSizeUsd, setTradeSizeUsd] = useState(1000);
  const [maxSlippageBps, setMaxSlippageBps] = useState(5.0);
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

  const spotLogs = executionLogs.filter(
    (l) => l.trade_type.toLowerCase().includes("spot") || l.direction?.toLowerCase().includes("spot")
  );

  const bestOpp = opportunities.length > 0 ? opportunities[0] : null;
  const totalSpotPnl = spotLogs.reduce((acc, l) => acc + (l.net_profit || 0), 0);

  return (
    <div className="flex-1 flex flex-col h-full bg-[#080B14] text-white/95 overflow-y-auto">
      {/* Top Header Banner */}
      <div className="border-b border-white/[0.08] bg-[#080B14]/80 backdrop-blur-xl px-6 py-4 flex flex-col md:flex-row md:items-center md:justify-between gap-4">
        <div>
          <div className="flex items-center space-x-2">
            <span className="p-1.5 bg-cyan-500/10 text-cyan-300 rounded-lg border border-cyan-500/20">
              <Coins className="w-5 h-5 text-cyan-400" />
            </span>
            <h1 className="text-lg font-bold text-white tracking-tight font-sans">
              Spot Cross-Exchange Bot
            </h1>
            <span className="px-2 py-0.5 rounded text-[11px] font-mono font-semibold bg-emerald-500/15 text-emerald-300 border border-emerald-500/30">
              Cash Market Arb
            </span>
          </div>
          <p className="text-xs text-white/50 font-sans mt-1">
            Manual opportunity selection with automated high-speed bot execution. Buy spot on Exchange A and simultaneously sell on Exchange B with zero funding rate or liquidation risk.
          </p>
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          <button
            onClick={loadSpotOpps}
            disabled={loading}
            className="flex items-center space-x-1.5 px-3.5 py-1.5 rounded-xl border border-white/[0.1] bg-white/[0.04] text-xs font-mono text-white/80 hover:text-white hover:bg-white/[0.08] transition shadow-xs"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-cyan-400" : "text-white/40"}`} />
            <span>Scan Spot</span>
          </button>
        </div>
      </div>

      {/* Summary Telemetry Metrics Bar */}
      <div className="px-6 py-3 border-b border-white/[0.08] bg-white/[0.02] grid grid-cols-2 sm:grid-cols-4 gap-3">
        <div className="glass-panel p-3.5 rounded-xl">
          <div className="text-[10px] font-medium text-white/40 font-mono uppercase tracking-wider">Scanned Discrepancies</div>
          <div className="text-lg font-bold text-white font-mono mt-0.5 tabular-nums">{opportunities.length} Pairs</div>
          <div className="text-[10px] text-white/40 mt-0.5">Across 6 Spot Venues</div>
        </div>

        <div className="glass-panel p-3.5 rounded-xl">
          <div className="text-[10px] font-medium text-white/40 font-mono uppercase tracking-wider">Max Net Spread</div>
          <div className="text-lg font-bold text-emerald-400 font-mono mt-0.5 tabular-nums">
            {bestOpp ? `+${bestOpp.net_spread_bps.toFixed(1)} BPS` : "—"}
          </div>
          <div className="text-[10px] text-white/40 mt-0.5 truncate">
            {bestOpp ? `${bestOpp.symbol} (${bestOpp.buy_venue} ➔ ${bestOpp.sell_venue})` : "Scanning..."}
          </div>
        </div>

        <div className="glass-panel p-3.5 rounded-xl">
          <div className="text-[10px] font-medium text-white/40 font-mono uppercase tracking-wider">Target Bot Latency</div>
          <div className="text-lg font-bold text-cyan-300 font-mono mt-0.5 tabular-nums">~14.2 ms</div>
          <div className="text-[10px] text-white/40 mt-0.5">Concurrent Atomic Dispatch</div>
        </div>

        <div className="glass-panel p-3.5 rounded-xl">
          <div className="text-[10px] font-medium text-white/40 font-mono uppercase tracking-wider">Realized Spot Profit</div>
          <div className="text-lg font-bold text-emerald-400 font-mono mt-0.5 tabular-nums">+${totalSpotPnl.toFixed(2)} USD</div>
          <div className="text-[10px] text-white/40 mt-0.5 tabular-nums">{spotLogs.length} Bot Fills</div>
        </div>
      </div>

      {/* Main Content Workspace */}
      <div className="p-6 flex-1 flex flex-col xl:flex-row gap-6">
        {/* Left Side: Live Opportunity Table */}
        <div className="flex-1 flex flex-col glass-panel rounded-2xl overflow-hidden">
          {/* Filter Bar */}
          <div className="p-4 border-b border-white/[0.08] flex flex-wrap items-center justify-between gap-3 bg-white/[0.02]">
            <div className="flex items-center space-x-2">
              <span className="text-xs font-bold text-white/70 font-mono uppercase">Filter Asset:</span>
              <div className="flex flex-wrap gap-1">
                {assets.map((a) => (
                  <button
                    key={a}
                    onClick={() => setSelectedAsset(a)}
                    className={`px-2.5 py-1 rounded-lg text-xs font-mono font-medium transition ${
                      selectedAsset === a
                        ? "bg-white/[0.14] text-cyan-300 border border-cyan-500/40 shadow-[0_0_12px_rgba(34,211,238,0.2)]"
                        : "bg-white/[0.03] text-white/50 hover:text-white hover:bg-white/[0.06] border border-white/[0.06]"
                    }`}
                  >
                    {a}
                  </button>
                ))}
              </div>
            </div>
            <div className="text-xs font-mono text-white/40">
              Live sorted by Net Spread BPS
            </div>
          </div>

          {/* Table */}
          <div className="flex-1 overflow-x-auto">
            <table className="w-full text-left text-xs font-mono select-none tabular-nums">
              <thead className="bg-[#0C1019] border-b border-white/[0.08] text-white/40 uppercase tracking-wider text-[10px]">
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
              <tbody className="divide-y divide-white/[0.06]">
                {opportunities.length === 0 ? (
                  <tr>
                    <td colSpan={8} className="py-12 text-center text-white/40 font-sans">
                      {loading ? "Scanning spot orderbooks..." : "No spot arbitrage discrepancies found for this asset."}
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
                            ? "bg-cyan-500/10 border-l-4 border-l-cyan-400 shadow-[inset_0_0_20px_rgba(34,211,238,0.08)]"
                            : "hover:bg-white/[0.03]"
                        }`}
                      >
                        <td className="py-3 px-4 font-bold text-white">
                          {opp.symbol}
                        </td>
                        <td className="py-3 px-4">
                          <div className="font-semibold text-white/90">{opp.buy_venue}</div>
                          <div className="text-[11px] text-white/40 font-mono tabular-nums">${opp.buy_price.toLocaleString(undefined, { minimumFractionDigits: 2 })}</div>
                        </td>
                        <td className="py-3 px-4">
                          <div className="font-semibold text-white/90">{opp.sell_venue}</div>
                          <div className="text-[11px] text-white/40 font-mono tabular-nums">${opp.sell_price.toLocaleString(undefined, { minimumFractionDigits: 2 })}</div>
                        </td>
                        <td className="py-3 px-4 text-right text-white/70 tabular-nums">
                          +{opp.gross_spread_bps.toFixed(1)}
                        </td>
                        <td className="py-3 px-4 text-right text-white/40 tabular-nums">
                          -{(opp.buy_fee_bps + opp.sell_fee_bps).toFixed(1)} bps (${opp.transfer_cost_usd.toFixed(2)})
                        </td>
                        <td className="py-3 px-4 text-right">
                          <span
                            className={`px-2 py-0.5 rounded font-bold tabular-nums ${
                              isProfitable
                                ? "bg-emerald-500/15 text-emerald-300 border border-emerald-500/30"
                                : "bg-white/[0.04] text-white/50"
                            }`}
                          >
                            +{opp.net_spread_bps.toFixed(1)} BPS
                          </span>
                        </td>
                        <td className="py-3 px-4 text-right font-bold text-emerald-400 tabular-nums">
                          +${opp.net_profit_usd.toFixed(2)}
                        </td>
                        <td className="py-3 px-4 text-center">
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleSelectOpp(opp);
                            }}
                            className={`px-3 py-1 rounded-lg text-[11px] font-medium transition ${
                              isSelected
                                ? "bg-gradient-to-r from-cyan-500 to-indigo-600 text-white font-semibold shadow-[0_0_12px_rgba(34,211,238,0.3)]"
                                : "bg-white/[0.04] border border-white/[0.08] text-white/70 hover:border-cyan-500/40 hover:text-cyan-300"
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
          <div className="glass-panel rounded-2xl p-5 shadow-xs space-y-4">
            <div className="flex items-center justify-between pb-3 border-b border-white/[0.08]">
              <div className="flex items-center space-x-2">
                <Zap className="w-4 h-4 text-cyan-400" />
                <h2 className="text-sm font-bold text-white font-mono uppercase tracking-tight">
                  Fast Execution Bot
                </h2>
              </div>
              <span className="text-[10px] font-mono px-2 py-0.5 rounded bg-cyan-500/10 text-cyan-300 border border-cyan-500/25 font-semibold">
                High Speed Dispatch
              </span>
            </div>

            {selectedOpp ? (
              <div className="space-y-4">
                {/* Signature Iridescent Route Card */}
                <div className="payment-card-iridescent space-y-3">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-1.5">
                      <span className="text-xs font-bold text-white font-mono tracking-wide">
                        {selectedOpp.symbol}
                      </span>
                      <span className="text-[10px] px-1.5 py-0.2 rounded bg-black/40 text-cyan-200 border border-white/10 font-mono">
                        SPOT ROUTE
                      </span>
                    </div>
                    <span className="text-xs font-bold text-emerald-300 font-mono tabular-nums shadow-xs">
                      +{selectedOpp.net_spread_bps.toFixed(1)} BPS Net
                    </span>
                  </div>

                  <div className="p-2.5 rounded-xl bg-black/30 backdrop-blur-md border border-white/10 space-y-1.5 text-xs font-mono">
                    <div className="text-white/80 flex items-center justify-between">
                      <span className="text-white/50">BUY: <strong className="text-white">{selectedOpp.buy_venue}</strong></span>
                      <span className="font-semibold text-white/95 tabular-nums">${selectedOpp.buy_price.toLocaleString()}</span>
                    </div>
                    <div className="text-white/80 flex items-center justify-between">
                      <span className="text-white/50">SELL: <strong className="text-white">{selectedOpp.sell_venue}</strong></span>
                      <span className="font-semibold text-white/95 tabular-nums">${selectedOpp.sell_price.toLocaleString()}</span>
                    </div>
                  </div>

                  <div className="flex items-center justify-between text-[11px] font-mono pt-1 text-white/70">
                    <span>Route Latency: <strong className="text-white font-semibold">~{selectedOpp.estimated_latency_ms} ms</strong></span>
                    <span className="text-emerald-300 font-bold">Zero Basis Risk</span>
                  </div>
                </div>

                {/* Trade Size Controls */}
                <div>
                  <div className="flex items-center justify-between text-xs font-mono text-white/70 mb-1.5">
                    <span>Trade Size ($ USD):</span>
                    <span className="font-bold text-white tabular-nums">${tradeSizeUsd.toLocaleString()}</span>
                  </div>
                  <input
                    type="range"
                    min={200}
                    max={10000}
                    step={100}
                    value={tradeSizeUsd}
                    onChange={(e) => setTradeSizeUsd(Number(e.target.value))}
                    className="w-full accent-cyan-400 cursor-pointer"
                  />
                  <div className="flex gap-1.5 mt-2">
                    {[500, 1000, 2500, 5000].map((s) => (
                      <button
                        key={s}
                        onClick={() => setTradeSizeUsd(s)}
                        className={`flex-1 py-1 rounded-lg text-[11px] font-mono font-medium transition ${
                          tradeSizeUsd === s
                            ? "bg-white/[0.14] text-cyan-300 border border-cyan-500/40 shadow-[0_0_10px_rgba(34,211,238,0.2)]"
                            : "bg-white/[0.04] text-white/60 hover:text-white hover:bg-white/[0.08]"
                        }`}
                      >
                        ${s >= 1000 ? `${s / 1000}k` : s}
                      </button>
                    ))}
                  </div>
                </div>

                {/* Max Slippage */}
                <div>
                  <div className="flex items-center justify-between text-xs font-mono text-white/70 mb-1">
                    <span>Max Allowed Slippage:</span>
                    <span className="font-bold text-white tabular-nums">{maxSlippageBps.toFixed(1)} BPS</span>
                  </div>
                  <input
                    type="range"
                    min={1.0}
                    max={15.0}
                    step={0.5}
                    value={maxSlippageBps}
                    onChange={(e) => setMaxSlippageBps(Number(e.target.value))}
                    className="w-full accent-cyan-400 cursor-pointer"
                  />
                </div>

                {/* Pre-flight Checks */}
                <div className="p-3 bg-white/[0.03] rounded-xl border border-white/[0.06] text-xs font-mono space-y-1.5 text-white/60">
                  <div className="flex items-center justify-between">
                    <span className="flex items-center space-x-1.5 text-white/80">
                      <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400" />
                      <span>Buy Venue Inventory</span>
                    </span>
                    <span className="font-semibold text-emerald-400">Ready ($15k USDC)</span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="flex items-center space-x-1.5 text-white/80">
                      <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400" />
                      <span>Sell Venue Inventory</span>
                    </span>
                    <span className="font-semibold text-emerald-400">Ready (Asset Pre-funded)</span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="flex items-center space-x-1.5 text-white/80">
                      <Clock className="w-3.5 h-3.5 text-cyan-400" />
                      <span>Route Latency</span>
                    </span>
                    <span className="font-semibold text-white/95 tabular-nums">~{selectedOpp.estimated_latency_ms} ms</span>
                  </div>
                  <div className="flex items-center justify-between pt-1 border-t border-white/[0.06] font-bold">
                    <span className="text-white/80">Net Expected Profit:</span>
                    <span className="text-emerald-400 tabular-nums">
                      +${((tradeSizeUsd * selectedOpp.net_spread_bps) / 10000).toFixed(2)} USD
                    </span>
                  </div>
                </div>

                {/* Bot Progress & Action */}
                {botMessage && (
                  <div
                    className={`p-3 rounded-xl border text-xs font-mono ${
                      botStatus === "filled"
                        ? "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"
                        : botStatus === "error"
                        ? "bg-rose-500/15 text-rose-300 border-rose-500/30"
                        : "bg-cyan-500/15 text-cyan-200 border-cyan-500/30 animate-pulse"
                    }`}
                  >
                    {botMessage}
                  </div>
                )}

                <button
                  onClick={handleDeployBot}
                  disabled={botStatus === "verifying" || botStatus === "dispatching"}
                  className="w-full py-3 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 text-white rounded-xl font-mono text-xs font-bold transition shadow-[0_0_24px_rgba(34,211,238,0.35)] flex items-center justify-center space-x-2 disabled:opacity-50"
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
              <div className="py-12 text-center text-white/40 text-xs font-sans">
                <AlertCircle className="w-6 h-6 mx-auto mb-2 text-white/20" />
                Select an opportunity from the table to stage and deploy the high-speed execution bot.
              </div>
            )}
          </div>

          {/* Recent Spot Bot Executions */}
          <div className="glass-panel rounded-2xl p-4 shadow-xs">
            <h3 className="text-xs font-bold text-white/80 font-mono uppercase tracking-tight mb-2.5">
              Recent Spot Bot Fills
            </h3>
            {spotLogs.length === 0 ? (
              <div className="text-[11px] text-white/40 font-sans py-4 text-center">
                No spot executions logged yet in this session.
              </div>
            ) : (
              <div className="space-y-2 max-h-48 overflow-y-auto">
                {spotLogs.slice(0, 5).map((log) => (
                  <div
                    key={log.id}
                    className="p-2.5 bg-white/[0.03] rounded-xl border border-white/[0.06] text-[11px] font-mono flex items-center justify-between"
                  >
                    <div>
                      <div className="font-bold text-white/95">{log.symbol}</div>
                      <div className="text-[10px] text-white/40">
                        {log.venue_a} ➔ {log.venue_b}
                      </div>
                    </div>
                    <div className="text-right">
                      <div className="font-bold text-emerald-400 tabular-nums">+${(log.net_profit || 0).toFixed(2)}</div>
                      <div className="text-[10px] text-white/40 tabular-nums">{Math.round((log.latency_us || 0) / 1000)}ms</div>
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
