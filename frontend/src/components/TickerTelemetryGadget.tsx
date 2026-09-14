import React, { useState, useMemo } from "react";
import {
  Activity,
  Sliders,
  CheckCircle2,
  AlertTriangle,
  Sparkles,
} from "lucide-react";
import type { ExecutionLogEntry, TickerStrategyConfigDto } from "../types";
import { formatCurrencyPnl } from "../types/tickers";

interface TickerTelemetryGadgetProps {
  symbol: string;
  logs: ExecutionLogEntry[];
  config?: TickerStrategyConfigDto;
}

export const TickerTelemetryGadget: React.FC<TickerTelemetryGadgetProps> = ({
  symbol,
  logs,
  config,
}) => {
  const [showTuningGuide, setShowTuningGuide] = useState<boolean>(true);

  // All executions for this ticker (unfiltered — telemetry aggregates everything)
  const tickerLogs = useMemo(() => {
    return logs.filter(
      (log) =>
        log.symbol === symbol ||
        log.symbol.startsWith(`${symbol}/`) ||
        log.symbol.endsWith(`/${symbol}`),
    );
  }, [logs, symbol]);

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
    <div className="bg-surface border border-surface-border rounded-lg p-5 shadow-xs transition mb-8">
      {/* Header Bar */}
      <div className="flex flex-col md:flex-row md:items-center justify-between pb-4 border-b border-surface-border gap-3">
        <div className="flex items-center space-x-2.5">
          <div className="p-2 rounded-md bg-blue-50 border border-blue-200 text-accent">
            <Activity className="w-4 h-4" />
          </div>
          <div>
            <div className="flex items-center space-x-2">
              <h3 className="text-sm font-bold font-mono tracking-wider text-slate-900">
                Execution Telemetry
              </h3>
              <span className="px-2 py-0.5 rounded-sm bg-surface-raised border border-surface-border text-xs font-mono font-medium text-slate-700">
                {symbol}
              </span>
            </div>
            <p className="text-xs text-slate-500 mt-0.5 font-sans">
              Aggregate trigger spread, baseline mean signal, and position step telemetry for quantitative post-trade diagnosis
            </p>
          </div>
        </div>

        <button
          onClick={() => setShowTuningGuide(!showTuningGuide)}
          className={`flex items-center space-x-1 px-2.5 py-1 text-xs font-mono rounded-md border transition self-start md:self-auto ${
            showTuningGuide
              ? "bg-blue-50 border-blue-200 text-accent font-medium"
              : "bg-surface-raised border-surface-border text-slate-600 hover:text-slate-900"
          }`}
          title="Toggle Parameter Tuning Insights"
        >
          <Sliders className="w-3.5 h-3.5" />
          <span>Tuning</span>
        </button>
      </div>

      {/* Ticker KPI Summary Bar */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3 py-4 text-xs font-mono">
        <div className="bg-surface-raised border border-surface-border rounded-md p-2.5">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Total Trades</div>
          <div className="text-sm font-bold text-slate-900 mt-1">
            {metrics.total} <span className="text-xs font-normal text-slate-500">({metrics.filled} filled)</span>
          </div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-md p-2.5">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Realized Net PnL</div>
          <div className={`text-sm font-bold mt-1 ${metrics.netPnl >= 0 ? "text-emerald-700" : "text-rose-700"}`}>
            {formatCurrencyPnl(metrics.netPnl)}
          </div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-md p-2.5">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Win / Fill Rate</div>
          <div className="text-sm font-bold text-slate-900 mt-1">{metrics.winRate.toFixed(1)}%</div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-md p-2.5">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Avg Trigger Spread</div>
          <div className="text-sm font-bold text-accent mt-1">
            {metrics.avgTrigger > 0 ? `+${metrics.avgTrigger.toFixed(1)} BPS` : "—"}
          </div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-md p-2.5">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Signal Baseline Mean</div>
          <div className="text-sm font-bold text-slate-700 mt-1">
            {metrics.avgBaseline > 0 ? `${metrics.avgBaseline.toFixed(1)} BPS` : "—"}
          </div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-md p-2.5">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Spread Dislocation Δ</div>
          <div className={`text-sm font-bold mt-1 ${metrics.avgDislocation >= 0 ? "text-emerald-700" : "text-slate-700"}`}>
            {metrics.avgDislocation !== 0 ? `+${metrics.avgDislocation.toFixed(1)} BPS` : "—"}
          </div>
        </div>
      </div>

      {/* Parameter Tuning Diagnostic Advice Banner */}
      {showTuningGuide && (
        <div className="p-3.5 bg-blue-50/50 border border-blue-200/80 rounded-md text-xs font-sans">
          <div className="flex items-center space-x-2 text-accent font-semibold text-xs mb-2">
            <Sparkles className="w-3.5 h-3.5" />
            <span>Parameter Tuning & Calibration Assistant</span>
            <span className="text-xs font-normal text-slate-500">
              (Target: {config?.targetProfitBps ?? 5.0} BPS | Percentile: {config?.windowPercentile ?? 50}% | Max Units: {config?.maxAccumulatedUnits ?? 3})
            </span>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-2 text-slate-700">
            {tuningInsights.map((insight, idx) => (
              <div key={idx} className="bg-white/80 p-2 rounded-sm border border-blue-100 flex items-start space-x-2">
                {insight.type === "success" && <CheckCircle2 className="w-4 h-4 text-emerald-600 shrink-0 mt-0.5" />}
                {insight.type === "warning" && <AlertTriangle className="w-4 h-4 text-amber-600 shrink-0 mt-0.5" />}
                {insight.type === "info" && <Sliders className="w-4 h-4 text-accent shrink-0 mt-0.5" />}
                <div>
                  <div className="font-semibold text-slate-900 text-xs">{insight.title}</div>
                  <div className="text-xs text-slate-600 mt-0.5 leading-relaxed">{insight.desc}</div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};
