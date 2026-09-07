import React, { useState, useMemo } from "react";
import {
  History,
  Sliders,
  CheckCircle2,
  AlertTriangle,
  ArrowRight,
  Layers,
  Sparkles,
  Search,
} from "lucide-react";
import type { ExecutionLogEntry, TickerStrategyConfigDto } from "../types";
import { formatCurrencyPnl } from "../types/tickers";

interface TickerTransactionHistoryProps {
  symbol: string;
  logs: ExecutionLogEntry[];
  config?: TickerStrategyConfigDto;
  onRefresh?: () => void;
}

export const TickerTransactionHistory: React.FC<TickerTransactionHistoryProps> = ({
  symbol,
  logs,
  config,
}) => {
  const [filterStatus, setFilterStatus] = useState<"ALL" | "FILLED" | "FAILED">("ALL");
  const [filterDirection, setFilterDirection] = useState<string>("ALL");
  const [showTuningGuide, setShowTuningGuide] = useState<boolean>(true);
  const [searchQuery, setSearchQuery] = useState<string>("");

  // Filter logs specifically relevant to this ticker
  const tickerLogs = useMemo(() => {
    return logs.filter((log) => {
      // Direct match on symbol (e.g. "BTC-USDC") or pair format (e.g. "BTC-USDC/ETH-USDC")
      const matchesSymbol =
        log.symbol === symbol ||
        log.symbol.startsWith(`${symbol}/`) ||
        log.symbol.endsWith(`/${symbol}`);

      if (!matchesSymbol) return false;

      if (filterStatus === "FILLED" && !log.success) return false;
      if (filterStatus === "FAILED" && log.success) return false;

      if (filterDirection !== "ALL") {
        if (filterDirection === "DIVERGENCE" && log.direction && !log.direction.toLowerCase().includes("divergence")) {
          return false;
        }
        if (filterDirection === "CONVERGENCE" && log.direction && !log.direction.toLowerCase().includes("convergence")) {
          return false;
        }
      }

      if (searchQuery.trim()) {
        const q = searchQuery.toLowerCase();
        const text = `${log.venue_a} ${log.venue_b} ${log.direction || ""} ${log.note}`.toLowerCase();
        if (!text.includes(q)) return false;
      }

      return true;
    });
  }, [logs, symbol, filterStatus, filterDirection, searchQuery]);

  // Aggregate Metrics for Active Ticker
  const metrics = useMemo(() => {
    const total = tickerLogs.length;
    const filled = tickerLogs.filter((l) => l.success).length;
    const winRate = total > 0 ? (filled / total) * 100 : 0;
    const netPnl = tickerLogs.reduce((acc, l) => acc + l.net_profit, 0);

    const validTriggers = tickerLogs.filter((l) => typeof l.trigger_spread_bps === "number");
    const avgTrigger =
      validTriggers.length > 0
        ? validTriggers.reduce((acc, l) => acc + (l.trigger_spread_bps || 0), 0) / validTriggers.length
        : 0;

    const validBaselines = tickerLogs.filter((l) => typeof l.baseline_mean_spread_bps === "number");
    const avgBaseline =
      validBaselines.length > 0
        ? validBaselines.reduce((acc, l) => acc + (l.baseline_mean_spread_bps || 0), 0) / validBaselines.length
        : 0;

    const avgDislocation = avgTrigger - avgBaseline;

    const maxStepReached = tickerLogs.reduce((max, l) => Math.max(max, l.position_step || 1), 0);

    return {
      total,
      filled,
      winRate,
      netPnl,
      avgTrigger,
      avgBaseline,
      avgDislocation,
      maxStepReached,
    };
  }, [tickerLogs]);

  // Quantitative Parameter Tuning Assessment
  const tuningInsights = useMemo(() => {
    const targetBps = config?.targetProfitBps ?? 5.0;
    const percentile = config?.windowPercentile ?? 50;
    const maxUnits = config?.maxAccumulatedUnits ?? 3;

    const recommendations: { title: string; desc: string; type: "info" | "warning" | "success" }[] = [];

    if (metrics.total === 0) {
      recommendations.push({
        title: "Awaiting Order Triggers",
        desc: `No transactions recorded yet for ${symbol}. When spreads exceed your nominal hurdle (${(targetBps + 3.5).toFixed(1)} BPS), orders will execute and record instantaneous trigger spread vs baseline signal.`,
        type: "info",
      });
      return recommendations;
    }

    // Spread Dislocation Analysis
    if (metrics.avgDislocation >= 5.0) {
      recommendations.push({
        title: "High Signal Quality (Strong Dislocation)",
        desc: `Average instantaneous spread at trigger (+${metrics.avgTrigger.toFixed(1)} BPS) is +${metrics.avgDislocation.toFixed(1)} BPS above baseline mean (+${metrics.avgBaseline.toFixed(1)} BPS). Current ${percentile}% percentile threshold is filtering noise effectively.`,
        type: "success",
      });
    } else if (metrics.avgDislocation > 0 && metrics.avgDislocation < 2.0) {
      recommendations.push({
        title: "Marginal Dislocation (Consider Raising Window Percentile)",
        desc: `Entries occur very close to rolling average (+${metrics.avgDislocation.toFixed(1)} BPS dislocation). Raising Window Percentile from ${percentile}% to 75% or 85% will prevent trading on small noise fluctuations.`,
        type: "warning",
      });
    }

    // Position Step & Grid Depth Analysis
    if (metrics.maxStepReached >= maxUnits) {
      recommendations.push({
        title: "Max Grid Units Reached",
        desc: `Strategy has scaled in to the max limit of ${maxUnits} units. Consider either widening the re-entry spacing or increasing Max Units if market liquidity supports deeper divergence accumulation.`,
        type: "warning",
      });
    } else {
      recommendations.push({
        title: "Healthy Grid Scaling",
        desc: `Max position step reached is Step ${metrics.maxStepReached} of ${maxUnits} units. Accumulation risk remains bounded within configured capital limits.`,
        type: "info",
      });
    }

    return recommendations;
  }, [metrics, config, symbol]);

  return (
    <div className="glass-panel rounded-2xl p-5 mb-8 text-white/95 space-y-4">
      {/* Header Bar */}
      <div className="flex flex-col md:flex-row md:items-center justify-between pb-4 border-b border-white/[0.08] gap-3">
        <div className="flex items-center space-x-2.5">
          <div className="p-2 rounded-xl bg-cyan-500/10 border border-cyan-500/20 text-cyan-300">
            <History className="w-4 h-4" />
          </div>
          <div>
            <div className="flex items-center space-x-2">
              <h3 className="text-sm font-semibold text-white tracking-tight">
                Transaction History & Execution Telemetry
              </h3>
              <span className="px-2 py-0.5 rounded bg-white/[0.04] border border-white/[0.08] text-xs font-mono font-medium text-cyan-300">
                {symbol}
              </span>
            </div>
            <p className="text-xs text-white/40 mt-0.5 font-sans">
              Instantaneous trigger spread, baseline mean signal, and position step telemetry for quantitative post-trade diagnosis
            </p>
          </div>
        </div>

        {/* Filter & Search Bar */}
        <div className="flex items-center space-x-2">
          <div className="relative">
            <Search className="w-3.5 h-3.5 text-white/40 absolute left-2.5 top-1/2 -translate-y-1/2" />
            <input
              type="text"
              placeholder="Filter venue/note..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-8 pr-2.5 py-1 text-xs font-mono bg-white/[0.04] border border-white/[0.08] rounded-lg text-white placeholder:text-white/30 focus:outline-none focus:ring-1 focus:ring-cyan-400 w-40"
            />
          </div>

          <div className="flex items-center bg-white/[0.03] p-0.5 rounded-lg border border-white/[0.08] text-xs font-mono">
            <button
              onClick={() => setFilterStatus("ALL")}
              className={`px-2 py-1 rounded transition ${
                filterStatus === "ALL" ? "bg-white/[0.12] text-cyan-300 font-medium shadow-[0_0_8px_rgba(34,211,238,0.2)]" : "text-white/50 hover:text-white"
              }`}
            >
              All ({tickerLogs.length})
            </button>
            <button
              onClick={() => setFilterStatus("FILLED")}
              className={`px-2 py-1 rounded transition ${
                filterStatus === "FILLED" ? "bg-emerald-500/20 text-emerald-300 font-medium" : "text-white/50 hover:text-white"
              }`}
            >
              Filled
            </button>
            <button
              onClick={() => setFilterStatus("FAILED")}
              className={`px-2 py-1 rounded transition ${
                filterStatus === "FAILED" ? "bg-rose-500/20 text-rose-300 font-medium" : "text-white/50 hover:text-white"
              }`}
            >
              Failed
            </button>
          </div>

          <div className="flex items-center bg-white/[0.03] px-2 py-1 rounded-lg border border-white/[0.08] text-xs font-mono">
            <select
              value={filterDirection}
              onChange={(e) => setFilterDirection(e.target.value)}
              className="bg-transparent text-white/80 font-mono text-xs focus:outline-none cursor-pointer"
            >
              <option value="ALL" className="bg-[#0C1019] text-white">All Directions</option>
              <option value="DIVERGENCE" className="bg-[#0C1019] text-white">Divergence Entry</option>
              <option value="CONVERGENCE" className="bg-[#0C1019] text-white">Convergence Unwind</option>
            </select>
          </div>

          <button
            onClick={() => setShowTuningGuide(!showTuningGuide)}
            className={`flex items-center space-x-1 px-2.5 py-1 text-xs font-mono rounded-lg border transition ${
              showTuningGuide
                ? "bg-cyan-500/10 border-cyan-500/30 text-cyan-300 font-medium"
                : "bg-white/[0.04] border-white/[0.08] text-white/50 hover:text-white"
            }`}
            title="Toggle Parameter Tuning Insights"
          >
            <Sliders className="w-3.5 h-3.5" />
            <span>Tuning</span>
          </button>
        </div>
      </div>

      {/* Ticker KPI Summary Bar */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3 py-2 border-b border-white/[0.06] text-xs font-mono tabular-nums">
        <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3">
          <div className="text-[10px] text-white/40 uppercase tracking-wider font-semibold">Total Trades</div>
          <div className="text-sm font-bold text-white mt-1">
            {metrics.total} <span className="text-[11px] font-normal text-white/40">({metrics.filled} filled)</span>
          </div>
        </div>

        <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3">
          <div className="text-[10px] text-white/40 uppercase tracking-wider font-semibold">Realized Net PnL</div>
          <div className={`text-sm font-bold mt-1 ${metrics.netPnl >= 0 ? "text-emerald-400" : "text-rose-400"}`}>
            {formatCurrencyPnl(metrics.netPnl)}
          </div>
        </div>

        <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3">
          <div className="text-[10px] text-white/40 uppercase tracking-wider font-semibold">Win / Fill Rate</div>
          <div className="text-sm font-bold text-white mt-1">{metrics.winRate.toFixed(1)}%</div>
        </div>

        <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3">
          <div className="text-[10px] text-white/40 uppercase tracking-wider font-semibold">Avg Trigger Spread</div>
          <div className="text-sm font-bold text-cyan-300 mt-1">
            {metrics.avgTrigger > 0 ? `+${metrics.avgTrigger.toFixed(1)} BPS` : "—"}
          </div>
        </div>

        <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3">
          <div className="text-[10px] text-white/40 uppercase tracking-wider font-semibold">Signal Baseline Mean</div>
          <div className="text-sm font-bold text-white/80 mt-1">
            {metrics.avgBaseline > 0 ? `${metrics.avgBaseline.toFixed(1)} BPS` : "—"}
          </div>
        </div>

        <div className="bg-white/[0.03] border border-white/[0.06] rounded-xl p-3">
          <div className="text-[10px] text-white/40 uppercase tracking-wider font-semibold">Spread Dislocation Δ</div>
          <div className={`text-sm font-bold mt-1 ${metrics.avgDislocation >= 0 ? "text-emerald-400" : "text-white/60"}`}>
            {metrics.avgDislocation !== 0 ? `+${metrics.avgDislocation.toFixed(1)} BPS` : "—"}
          </div>
        </div>
      </div>

      {/* Parameter Tuning Diagnostic Advice Banner */}
      {showTuningGuide && (
        <div className="p-4 bg-white/[0.03] border border-white/[0.08] rounded-xl text-xs font-sans">
          <div className="flex items-center space-x-2 text-cyan-300 font-semibold text-xs mb-2.5">
            <Sparkles className="w-3.5 h-3.5" />
            <span>Parameter Tuning & Calibration Assistant</span>
            <span className="text-[11px] font-normal text-white/40">
              (Target: {config?.targetProfitBps ?? 5.0} BPS | Percentile: {config?.windowPercentile ?? 50}% | Max Units: {config?.maxAccumulatedUnits ?? 3})
            </span>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-2.5 text-white/80">
            {tuningInsights.map((insight, idx) => (
              <div key={idx} className="bg-white/[0.02] p-3 rounded-xl border border-white/[0.06] flex items-start space-x-2.5">
                {insight.type === "success" && <CheckCircle2 className="w-4 h-4 text-emerald-400 shrink-0 mt-0.5" />}
                {insight.type === "warning" && <AlertTriangle className="w-4 h-4 text-amber-400 shrink-0 mt-0.5" />}
                {insight.type === "info" && <Sliders className="w-4 h-4 text-cyan-400 shrink-0 mt-0.5" />}
                <div>
                  <div className="font-semibold text-white text-[11px]">{insight.title}</div>
                  <div className="text-[11px] text-white/60 mt-0.5 leading-relaxed">{insight.desc}</div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* History Table */}
      <div className="overflow-x-auto rounded-xl border border-white/[0.08]">
        <table className="w-full text-left text-xs font-mono border-collapse tabular-nums">
          <thead>
            <tr className="border-b border-white/[0.08] text-white/40 text-[10px] uppercase tracking-wider bg-[#0C1019]">
              <th className="py-2.5 px-3 font-semibold">DateTime</th>
              <th className="py-2.5 px-3 font-semibold">Direction & Action</th>
              <th className="py-2.5 px-3 font-semibold">Venues (Buy ➔ Sell)</th>
              <th className="py-2.5 px-3 font-semibold text-right">Trigger Spread</th>
              <th className="py-2.5 px-3 font-semibold text-right">Signal Baseline (Mean & Δ)</th>
              <th className="py-2.5 px-3 font-semibold text-center">Position Step</th>
              <th className="py-2.5 px-3 font-semibold text-right">Nominal Hurdle</th>
              <th className="py-2.5 px-3 font-semibold text-right">Net PnL</th>
              <th className="py-2.5 px-3 font-semibold">Status / Diagnostic Note</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-white/[0.06]">
            {tickerLogs.length === 0 ? (
              <tr>
                <td colSpan={9} className="py-8 text-center text-white/40 font-sans text-xs">
                  No execution transactions found for <span className="font-mono font-medium text-cyan-300">{symbol}</span> matching the active filters.
                </td>
              </tr>
            ) : (
              tickerLogs.map((log) => {
                const date = new Date(log.timestamp);
                const dateFormatted = date.toLocaleDateString("en-US", {
                  month: "short",
                  day: "numeric",
                });
                const timeFormatted = date.toLocaleTimeString("en-US", {
                  hour: "2-digit",
                  minute: "2-digit",
                  second: "2-digit",
                  hour12: false,
                });

                const triggerSpread = log.trigger_spread_bps;
                const baselineMean = log.baseline_mean_spread_bps;
                const dislocation =
                  typeof triggerSpread === "number" && typeof baselineMean === "number"
                    ? triggerSpread - baselineMean
                    : null;

                const hurdle = log.nominal_hurdle_bps;
                const hurdleBuffer =
                  typeof triggerSpread === "number" && typeof hurdle === "number"
                    ? triggerSpread - hurdle
                    : null;

                const step = log.position_step ?? 1;
                const maxUnits = log.max_units ?? config?.maxAccumulatedUnits ?? 3;

                const isConvergence = log.direction?.toLowerCase().includes("convergence");
                const isCarry = log.trade_type === "Funding" || log.direction?.toLowerCase().includes("carry");

                return (
                  <tr key={log.id} className="hover:bg-white/[0.03] transition">
                    {/* DateTime */}
                    <td className="py-2.5 px-3 text-white/70 whitespace-nowrap">
                      <div className="font-medium text-white">{timeFormatted}</div>
                      <div className="text-[10px] text-white/40">{dateFormatted}</div>
                    </td>

                    {/* Direction & Action */}
                    <td className="py-2.5 px-3 whitespace-nowrap">
                      <div className="flex items-center space-x-1.5">
                        <span
                          className={`px-2 py-0.5 rounded text-[10px] font-mono font-medium border ${
                            isConvergence
                              ? "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"
                              : isCarry
                              ? "bg-indigo-500/15 text-indigo-300 border-indigo-500/30"
                              : "bg-cyan-500/15 text-cyan-300 border-cyan-500/30"
                          }`}
                        >
                          {log.direction || "Divergence Entry"}
                        </span>
                      </div>
                    </td>

                    {/* Venues */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-white/80">
                      <div className="flex items-center space-x-1.5">
                        <span className="font-medium text-white">{log.venue_a}</span>
                        <ArrowRight className="w-3 h-3 text-white/30" />
                        <span className="font-medium text-white">{log.venue_b}</span>
                      </div>
                    </td>

                    {/* Trigger Spread */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      {typeof triggerSpread === "number" ? (
                        <div>
                          <div className="font-bold text-white tabular-nums">
                            {triggerSpread > 0 ? `+${triggerSpread.toFixed(1)}` : triggerSpread.toFixed(1)} BPS
                          </div>
                          <div className="text-[10px] text-white/40">instantaneous</div>
                        </div>
                      ) : (
                        <span className="text-white/30">—</span>
                      )}
                    </td>

                    {/* Average Spread Baseline Signal */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      {typeof baselineMean === "number" ? (
                        <div>
                          <div className="text-white/80 font-medium tabular-nums">{baselineMean.toFixed(1)} BPS</div>
                          {dislocation !== null && (
                            <div
                              className={`text-[10px] font-medium tabular-nums ${
                                dislocation >= 0 ? "text-emerald-400" : "text-rose-400"
                              }`}
                            >
                              Δ {dislocation >= 0 ? `+${dislocation.toFixed(1)}` : dislocation.toFixed(1)} BPS
                            </div>
                          )}
                        </div>
                      ) : (
                        <span className="text-white/30">—</span>
                      )}
                    </td>

                    {/* Position Step Number */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-center">
                      <div className="inline-flex items-center space-x-1 px-2 py-0.5 rounded bg-white/[0.04] border border-white/[0.08] text-[11px] font-mono font-medium text-white/80">
                        <Layers className="w-3 h-3 text-white/40" />
                        <span>
                          Step {step} / {maxUnits}
                        </span>
                      </div>
                    </td>

                    {/* Nominal Hurdle & Margin */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      {typeof hurdle === "number" ? (
                        <div>
                          <div className="text-white/80 tabular-nums">{hurdle.toFixed(1)} BPS</div>
                          {hurdleBuffer !== null && (
                            <div
                              className={`text-[10px] tabular-nums ${
                                hurdleBuffer >= 0 ? "text-emerald-400" : "text-rose-400"
                              }`}
                            >
                              Margin {hurdleBuffer >= 0 ? `+${hurdleBuffer.toFixed(1)}` : hurdleBuffer.toFixed(1)}
                            </div>
                          )}
                        </div>
                      ) : (
                        <span className="text-white/30">—</span>
                      )}
                    </td>

                    {/* Net PnL */}
                    <td className="py-2.5 px-3 whitespace-nowrap text-right">
                      <div className={`font-bold tabular-nums ${log.net_profit >= 0 ? "text-emerald-400" : "text-rose-400"}`}>
                        {formatCurrencyPnl(log.net_profit)}
                      </div>
                      <div className="text-[10px] text-white/40 tabular-nums">{log.latency_us} µs</div>
                    </td>

                    {/* Diagnostic Note / Status */}
                    <td className="py-2.5 px-3 text-white/60 max-w-xs truncate">
                      <div className="flex items-center space-x-1.5">
                        {log.success ? (
                          <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 shadow-[0_0_6px_rgba(52,211,153,0.8)] shrink-0" />
                        ) : (
                          <span className="w-1.5 h-1.5 rounded-full bg-rose-400 shadow-[0_0_6px_rgba(251,113,133,0.8)] shrink-0" />
                        )}
                        <span className="truncate text-[11px] text-white/60" title={log.note}>
                          {log.note}
                        </span>
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
};
