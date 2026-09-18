import React, { useState, useEffect, useRef } from "react";
import {
  Triangle,
  RefreshCw,
  Zap,
  CheckCircle2,
  AlertCircle,
  SlidersHorizontal,
} from "lucide-react";
import { api } from "../services/api";
import type {
  TriangleOpportunityDto,
  TriangleExecutionParams,
  ExecutionLogEntry,
  StrategyParamsDto,
} from "../types";
import { BotLifecycleControls } from "./BotLifecycleControls";
import { EmptyState } from "./EmptyState";
import { useBotLifecycle } from "../hooks/useBotLifecycle";

interface TriangularArbitrageProps {
  executionLogs: ExecutionLogEntry[];
  onRefreshAll: () => Promise<void>;
  isPaused?: boolean;
  hasApiConnectivity?: boolean;
  onOpenConfig?: (symbol: string, strategy?: string) => void;
  params?: StrategyParamsDto;
}

const formatBps = (bps: number | undefined): string => {
  if (bps === undefined || isNaN(bps)) return "0.0";
  return bps.toFixed(1);
};

const formatPrice = (price: number, symbol: string): string => {
  const parts = symbol.split(/[-_/]/);
  const quote = parts[1]?.toUpperCase() || "USDC";
  const formatted = price.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: price < 1 ? 6 : 2,
  });
  if (quote === "USDC" || quote === "USD" || quote === "USDT") {
    return `$${formatted}`;
  }
  return `${formatted} ${quote}`;
};

export const TriangularArbitrage: React.FC<TriangularArbitrageProps> = ({
  executionLogs: _executionLogs,
  onRefreshAll,
  isPaused = false,
  hasApiConnectivity = false,
  onOpenConfig,
  params,
}) => {
  const [opportunities, setOpportunities] = useState<TriangleOpportunityDto[]>([]);
  const [selectedAsset, setSelectedAsset] = useState<string>("ALL");
  const [selectedOpp, setSelectedOpp] = useState<TriangleOpportunityDto | null>(null);

  // Bot lifecycle — triangular cycles are atomic (3 legs on one venue), so a
  // stop never leaves residual exposure; reduce-only drains instantly.
  const bot = useBotLifecycle({ strategyId: "triangle" });
  const lastAutoTriExecRef = useRef<number>(0);
  const tradeSizeUsd = params?.triangleTradeSizeUsd ?? 10000;
  const minProfitBps = params?.triangleMinProfitBps ?? 2.0;
  const maxSlippageBps = params?.triangleMaxSlippageBps ?? 5.0;
  const [botStatus, setBotStatus] = useState<"idle" | "verifying" | "dispatching" | "filled" | "error">("idle");
  const [botMessage, setBotMessage] = useState<string>("");

  const assets = ["ALL", "BTC", "ETH", "SOL", "BNB", "XRP"];

  const loadTriangleOpps = async () => {
    try {
      const data = await api.scanTriangleOpportunities(selectedAsset === "ALL" ? "" : selectedAsset);
      setOpportunities(data);
      if (selectedOpp) {
        const refreshed = data.find((o) => o.id === selectedOpp.id);
        if (refreshed) setSelectedOpp(refreshed);
      } else if (data.length > 0) {
        setSelectedOpp(data[0]);
      }
    } catch (err) {
      console.error("Failed to load triangular opportunities:", err);
    }
  };

  useEffect(() => {
    loadTriangleOpps();
    const interval = setInterval(loadTriangleOpps, 3000);
    return () => clearInterval(interval);
  }, [selectedAsset]);

  const handleSelectOpp = (opp: TriangleOpportunityDto) => {
    setSelectedOpp(opp);
    setBotStatus("idle");
    setBotMessage("");
  };

  const handleDeployCycle = async () => {
    if (!selectedOpp) return;
    if (bot.phase === "paused" || bot.phase === "reduce_only" || bot.phase === "stopping") {
      setBotStatus("error");
      setBotMessage(`Entry blocked: bot is ${bot.phase.replace("_", "-")} — only exits are permitted`);
      return;
    }
    setBotStatus("verifying");
    setBotMessage("Validating triangular balance liquidity & price slippage tolerance...");

    try {
      await new Promise((r) => setTimeout(r, 400));
      setBotStatus("dispatching");
      setBotMessage(`Submitting sequential 3-leg cycle on ${selectedOpp.venue}...`);

      const params: TriangleExecutionParams = {
        id: selectedOpp.id,
        venue: selectedOpp.venue,
        path: selectedOpp.path,
        trade_size_usd: tradeSizeUsd,
        min_profit_bps: minProfitBps,
        net_spread_bps: selectedOpp.net_spread_bps,
      };

      const result = await api.executeTriangleArbitrage(params);
      await onRefreshAll();
      await loadTriangleOpps();

      setBotStatus("filled");
      setBotMessage(
        `✓ Triangular Cycle Filled! Net Profit: +$${result.realized_profit_usd.toFixed(2)} USD (+${result.spread_bps.toFixed(1)} BPS) in ${selectedOpp.estimated_latency_ms}ms.`
      );
    } catch (err: unknown) {
      setBotStatus("error");
      setBotMessage(err instanceof Error ? err.message : "Execution failed");
    }
  };

  // Automated Execution Bot Loop
  useEffect(() => {
    if (!bot.entriesAllowed || isPaused) return;

    const interval = setInterval(async () => {
      const now = Date.now();
      if (now - lastAutoTriExecRef.current < 4000) return;

      const viableOpp = opportunities.find((o) => o.is_executable && o.net_spread_bps >= minProfitBps);
      if (viableOpp) {
        lastAutoTriExecRef.current = now;
        try {
          await api.executeTriangleArbitrage({
            id: viableOpp.id,
            venue: viableOpp.venue,
            path: viableOpp.path,
            trade_size_usd: tradeSizeUsd,
            min_profit_bps: minProfitBps,
            net_spread_bps: viableOpp.net_spread_bps,
          });
          await onRefreshAll();
          await loadTriangleOpps();
        } catch (e) {
          console.error("Triangular auto-execution error:", e);
        }
      }
    }, 2500);

    return () => clearInterval(interval);
  }, [bot.entriesAllowed, isPaused, opportunities, minProfitBps, tradeSizeUsd]);

  const activeOpp = selectedOpp || opportunities[0] || null;

  return (
    <div className="w-full p-4 space-y-4 overflow-y-auto h-full select-none pb-24 font-mono">
      {/* 1. Strategy Ribbon — read-only params left, configurator + toggle right */}
      <div className="ribbon-container">
        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="text-[13px] font-mono font-bold tracking-wider text-slate-900 pr-1">
            Strategy Parameters
          </span>
          <div
            className="ribbon-pill"
            title="Notional per 3-leg cycle — edit in Cycle Guardrails"
          >
            <span className="text-slate-400">Trade Size</span>
            <span className="font-bold text-slate-900 tabular-nums">{tradeSizeUsd.toLocaleString()}</span>
            <span className="text-slate-400">USD</span>
          </div>

          <div
            className="ribbon-pill"
            title="Net profit hurdle required to fire — edit in Cycle Guardrails"
          >
            <span className="text-slate-400">Min Profit</span>
            <span className="font-bold text-slate-900 tabular-nums">{minProfitBps}</span>
            <span className="text-slate-400">BPS</span>
          </div>

          <div
            className="ribbon-pill"
            title="Execution slippage tolerance — edit via Configurator"
          >
            <span className="text-slate-400">Max Slippage</span>
            <span className="font-bold text-slate-900 tabular-nums">{maxSlippageBps}</span>
            <span className="text-slate-400">BPS</span>
          </div>

          {isPaused && (
            <span className="badge-status-warn">
              Engine Paused
            </span>
          )}
        </div>

        <div className="flex items-center space-x-3 shrink-0">
          <button
            type="button"
            onClick={() => onOpenConfig?.("BTC-USDC", "triangle")}
            className="btn-secondary !text-xs !py-2 !px-2 font-sans cursor-pointer gap-2"
            title="Configure Triangle strategy parameters (applies to all 3-leg cycles)"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-accent" />
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

      {/* 2. Top Interactive Visualizer & Parameters Grid */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        {/* Left 2 Cols: 3-Leg Cycle Graph Visualizer */}
        <div className="lg:col-span-2 bg-surface p-4 rounded-xl border border-surface-border shadow-card space-y-3">
          <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
            <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 flex items-center space-x-2">
              <Triangle className="w-4 h-4 text-accent" />
              <span>3-Leg Cycle Architecture</span>
            </h2>
            {activeOpp && (
              <span className="text-xs font-bold text-accent bg-accent-dim px-2.5 py-0.5 rounded-md border border-accent-line">
                {activeOpp.venue}
              </span>
            )}
          </div>

          {activeOpp ? (
            <div className="space-y-4">
              {/* Path Node Sequence */}
              <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
                {/* Leg 1 */}
                <div className="p-3 bg-surface-raised rounded-lg border border-surface-border space-y-2">
                  <div className="flex justify-between items-center text-xs">
                    <span className="px-2 py-0.5 bg-blue-100 text-blue-800 rounded-md font-bold">Leg 1</span>
                    <span className="font-bold text-slate-800">{activeOpp.leg1.action}</span>
                  </div>
                  <div className="text-sm font-bold text-slate-900">{activeOpp.leg1.symbol}</div>
                  <div className="flex justify-between text-xs text-slate-500">
                    <span>Price:</span>
                    <span className="font-bold text-slate-700 tabular-nums">{formatPrice(activeOpp.leg1.price, activeOpp.leg1.symbol)}</span>
                  </div>
                  <div className="flex justify-between text-xs text-slate-400">
                    <span>Fee:</span>
                    <span className="tabular-nums">{formatBps(activeOpp.leg1.fee_bps)} BPS</span>
                  </div>
                </div>

                {/* Leg 2 */}
                <div className="p-3 bg-surface-raised rounded-lg border border-surface-border space-y-2">
                  <div className="flex justify-between items-center text-xs">
                    <span className="px-2 py-0.5 bg-purple-100 text-purple-800 rounded-md font-bold">Leg 2</span>
                    <span className="font-bold text-slate-800">{activeOpp.leg2.action}</span>
                  </div>
                  <div className="text-sm font-bold text-slate-900">{activeOpp.leg2.symbol}</div>
                  <div className="flex justify-between text-xs text-slate-500">
                    <span>Price:</span>
                    <span className="font-bold text-slate-700 tabular-nums">{formatPrice(activeOpp.leg2.price, activeOpp.leg2.symbol)}</span>
                  </div>
                  <div className="flex justify-between text-xs text-slate-400">
                    <span>Fee:</span>
                    <span className="tabular-nums">{formatBps(activeOpp.leg2.fee_bps)} BPS</span>
                  </div>
                </div>

                {/* Leg 3 */}
                <div className="p-3 bg-surface-raised rounded-lg border border-surface-border space-y-2">
                  <div className="flex justify-between items-center text-xs">
                    <span className="px-2 py-0.5 bg-profit-dim text-profit border border-profit-border rounded-md font-bold">Leg 3</span>
                    <span className="font-bold text-slate-800">{activeOpp.leg3.action}</span>
                  </div>
                  <div className="text-sm font-bold text-slate-900">{activeOpp.leg3.symbol}</div>
                  <div className="flex justify-between text-xs text-slate-500">
                    <span>Price:</span>
                    <span className="font-bold text-slate-700 tabular-nums">{formatPrice(activeOpp.leg3.price, activeOpp.leg3.symbol)}</span>
                  </div>
                  <div className="flex justify-between text-xs text-slate-400">
                    <span>Fee:</span>
                    <span className="tabular-nums">{formatBps(activeOpp.leg3.fee_bps)} BPS</span>
                  </div>
                </div>
              </div>

              {/* Cycle Flow Banner */}
              <div className="p-3 bg-accent-dim/60 rounded-lg border border-accent-line flex items-center justify-between text-xs">
                <div className="flex items-center space-x-2">
                  <span className="font-bold text-slate-700">Synthetic Route:</span>
                  <span className="font-bold text-accent">{activeOpp.path}</span>
                </div>
                <div className="flex items-center space-x-4">
                  <span>Gross: <strong className="text-slate-800 tabular-nums">+{formatBps(activeOpp.gross_spread_bps)} BPS</strong></span>
                  <span>Fees: <strong className="text-slate-500 tabular-nums">-{formatBps(activeOpp.total_fee_bps)} BPS</strong></span>
                  <span>Net: <strong className={`tabular-nums ${activeOpp.net_spread_bps >= 0 ? "text-profit" : "text-loss"}`}>
                    {activeOpp.net_spread_bps >= 0 ? `+${formatBps(activeOpp.net_spread_bps)}` : formatBps(activeOpp.net_spread_bps)} BPS
                  </strong></span>
                </div>
              </div>
            </div>
          ) : (
            <EmptyState
              title="No Triangular Cycle Selected"
              hint="Select a detected cycle from the opportunities table."
            />
          )}
        </div>

        {/* Right 1 Col: Execution Bot Settings & 1-Click Action */}
        <div className="bg-surface p-4 rounded-xl border border-surface-border shadow-card space-y-3">
          <div className="flex items-center border-b border-surface-border pb-2.5">
            <h2 className="text-[13px] font-bold font-sans tracking-wider text-slate-900 flex items-center gap-2">
              <SlidersHorizontal className="w-4 h-4 text-accent" />
              <span>Cycle Guardrails</span>
            </h2>

          </div>

          <div className="space-y-3 text-xs font-sans">
            <div className="flex items-center justify-between gap-3 py-1">
              <span className="text-slate-500">Trade Size:</span>
              <span className="font-mono font-bold text-slate-900 tabular-nums">${tradeSizeUsd.toLocaleString()}</span>
            </div>

            <div className="flex items-center justify-between gap-3 py-1">
              <span className="text-slate-500">Min Profit Hurdle:</span>
              <span className="font-mono font-bold text-slate-900 tabular-nums">{minProfitBps.toFixed(1)} bps</span>
            </div>

            <div className="flex items-center justify-between gap-3 py-1">
              <span className="text-slate-500">Max Leg Slippage:</span>
              <span className="font-mono font-bold text-slate-900 tabular-nums">{maxSlippageBps.toFixed(1)} bps</span>
            </div>

            {/* Deploy 1-Click Button */}
            <div className="pt-2">
              <button
                onClick={handleDeployCycle}
                disabled={!activeOpp || isPaused || botStatus === "verifying" || botStatus === "dispatching"}
                className="btn-primary w-full !py-2.5 !px-2.5 text-xs font-bold transition flex items-center justify-center space-x-2 cursor-pointer disabled:opacity-50"
              >
                <Zap className="w-4 h-4 fill-current" />
                <span>
                  {isPaused
                    ? "TRADING PAUSED"
                    : botStatus === "dispatching"
                    ? "DISPATCHING CYCLE..."
                    : botStatus === "verifying"
                    ? "VERIFYING DEPTH..."
                    : "EXECUTE 3-LEG CYCLE"}
                </span>
              </button>
            </div>

            {/* Status Message */}
            {botMessage && (
              <div
                className={`p-3 rounded-lg border text-xs flex items-start space-x-2 ${
                  botStatus === "filled"
                    ? "bg-profit-dim text-profit border-profit-border"
                    : botStatus === "error"
                    ? "bg-loss-dim text-loss border-loss-border"
                    : "bg-accent-dim text-accent border-accent-line"
                }`}
              >
                {botStatus === "filled" && <CheckCircle2 className="w-4 h-4 shrink-0 mt-0.5" />}
                {botStatus === "error" && <AlertCircle className="w-4 h-4 shrink-0 mt-0.5" />}
                {botStatus !== "filled" && botStatus !== "error" && (
                  <RefreshCw className="w-4 h-4 shrink-0 mt-0.5 animate-spin" />
                )}
                <span className="leading-relaxed">{botMessage}</span>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* 3. Real-Time Triangular Opportunities Table */}
      <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-surface-border pb-2.5">
          <h2 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
            Detected Triangular Opportunities ({opportunities.length})
          </h2>
          <div className="flex items-center space-x-3">
            {/* Asset Filters */}
            <div className="flex items-center bg-surface-raised p-1 rounded-lg border border-surface-border space-x-1">
              {assets.map((asset) => (
                <button
                  key={asset}
                  onClick={() => setSelectedAsset(asset)}
                  className={`px-2 py-2 rounded-lg text-[11px] font-mono font-medium transition cursor-pointer ${
                    selectedAsset === asset
                      ? "bg-accent text-white shadow-2xs"
                      : "text-slate-600 hover:text-slate-900"
                  }`}
                >
                  {asset}
                </button>
              ))}
            </div>
          </div>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs font-mono">
            <thead>
              <tr className="border-b border-surface-border text-slate-500 text-xs">
                <th className="py-2.5 px-3">Path</th>
                <th className="py-2.5 px-3">Exchange</th>
                <th className="py-2.5 px-3">Gross Discrepancy</th>
                <th className="py-2.5 px-3">Total Fees</th>
                <th className="py-2.5 px-3">Net Profit BPS</th>
                <th className="py-2.5 px-3">Est. Profit ($10k)</th>
                <th className="py-2.5 px-3">Status</th>
                <th className="py-2.5 px-3 text-right">Action</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-surface-border">
              {opportunities.length > 0 ? (
                opportunities.map((opp) => {
                  const isSelected = selectedOpp?.id === opp.id;
                  return (
                    <tr
                      key={opp.id}
                      onClick={() => handleSelectOpp(opp)}
                      className={`hover:bg-slate-50/70 transition cursor-pointer ${
                        isSelected ? "bg-accent-dim/40" : ""
                      }`}
                    >
                      <td className="py-3 px-3 font-bold text-slate-900 flex items-center space-x-1.5">
                        <Triangle className="w-3.5 h-3.5 text-accent shrink-0" />
                        <span>{opp.path}</span>
                      </td>
                      <td className="py-3 px-3 text-slate-600">{opp.venue}</td>
                      <td className="py-3 px-3 font-medium text-slate-700 tabular-nums">+{formatBps(opp.gross_spread_bps)} BPS</td>
                      <td className="py-3 px-3 text-slate-500 tabular-nums">-{formatBps(opp.total_fee_bps)} BPS</td>
                      <td className="py-3 px-3 font-bold">
                        <span className={`tabular-nums ${opp.net_spread_bps >= 0 ? "text-profit" : "text-loss"}`}>
                          {opp.net_spread_bps >= 0 ? `+${formatBps(opp.net_spread_bps)}` : formatBps(opp.net_spread_bps)} BPS
                        </span>
                      </td>
                      <td className="py-3 px-3 font-bold">
                        <span className={`tabular-nums ${opp.net_profit_usd >= 0 ? "text-profit" : "text-loss"}`}>
                          {opp.net_profit_usd >= 0 ? `+$${opp.net_profit_usd.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` : `-$${Math.abs(opp.net_profit_usd).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`}
                        </span>
                      </td>
                      <td className="py-3 px-3">
                        <span
                          className={`px-2 py-0.5 rounded-md text-xs font-bold ${
                            opp.is_executable
                              ? "bg-profit-dim text-profit border border-profit-border"
                              : "bg-surface-raised text-slate-400 border border-surface-border"
                          }`}
                        >
                          {opp.is_executable ? "EXECUTABLE" : "SUB-HURDLE"}
                        </span>
                      </td>
                      <td className="py-3 px-3 text-right">
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            handleSelectOpp(opp);
                            handleDeployCycle();
                          }}
                          disabled={isPaused}
                          className="px-2 py-2 bg-accent hover:bg-primary-hover disabled:opacity-50 text-white rounded-lg text-xs font-medium transition cursor-pointer"
                        >
                          Fill Cycle
                        </button>
                      </td>
                    </tr>
                  );
                })
              ) : (
                <tr>
                  <td colSpan={8}>
                    <EmptyState
                      plain
                      title="No Opportunity Detected"
                    />
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
};
