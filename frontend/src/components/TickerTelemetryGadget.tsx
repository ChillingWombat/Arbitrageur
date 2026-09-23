import React, { useMemo } from "react";

import type { ExecutionLogEntry } from "../types";
import { formatCurrencyPnl } from "../types/tickers";

interface TickerTelemetryGadgetProps {
  symbol: string;
  logs: ExecutionLogEntry[];
  gridCurrentUnits?: number;
  gridMaxUnits?: number;
}

export const TickerTelemetryGadget: React.FC<TickerTelemetryGadgetProps> = ({
  symbol,
  logs,
  gridCurrentUnits = 0,
  gridMaxUnits = 0,
}) => {
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

  return (
    <div className="bg-surface border border-surface-border rounded-xl p-3.5 shadow-card transition">
      {/* Ticker KPI Summary Bar */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3 text-xs font-mono">
        <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 shadow-xs">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Total Cycles</div>
          <div className="text-sm font-bold text-slate-900 tabular-nums mt-1">
            {metrics.total} <span className="text-xs font-normal text-slate-500">({metrics.filled} closed)</span>
          </div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 shadow-xs">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Total Net Profit</div>
          <div className={`text-sm font-bold tabular-nums mt-1 ${metrics.netPnl >= 0 ? "text-profit" : "text-loss"}`}>
            {formatCurrencyPnl(metrics.netPnl)}
          </div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 shadow-xs" title="Rate of successful cycles — entered and exited after reaching target">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Success Rate</div>
          <div className="text-sm font-bold text-slate-900 tabular-nums mt-1">{metrics.winRate.toFixed(1)}%</div>
        </div>

        <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 shadow-xs">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Avg Trigger Spread</div>
          <div className="text-sm font-bold text-accent-strong tabular-nums mt-1">
            {metrics.avgTrigger > 0 ? `+${metrics.avgTrigger.toFixed(1)} BPS` : "—"}
          </div>
        </div>


        <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 shadow-xs">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Current Dislocation</div>
          <div className={`text-sm font-bold tabular-nums mt-1 ${metrics.avgDislocation >= 0 ? "text-profit" : "text-slate-700"}`}>
            {metrics.avgDislocation !== 0 ? `+${metrics.avgDislocation.toFixed(1)} BPS` : "—"}
          </div>
        </div>


        <div className="bg-surface-raised border border-surface-border rounded-lg p-2.5 shadow-xs">
          <div className="text-xs text-slate-500 tracking-wider font-semibold">Grid Step</div>
          <div className="text-sm font-bold text-slate-900 tabular-nums mt-1">
            {gridCurrentUnits}/{gridMaxUnits} steps
          </div>
        </div>
      </div>
    </div>
  );
};
