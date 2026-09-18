import React, { useState, useEffect } from "react";
import {
  TrendingUp,
  Award,
  Activity,
  BarChart2,
  RefreshCw,
  Cpu,
  CheckCircle2,
  Sliders,
  Percent,
  Target,
  Sigma,
} from "lucide-react";
import { SpreadGapIcon } from "./icons";
import { formatStrategyName } from "../types/tickers";
import { api } from "../services/api";
import type { StrategyMetrics, WfoOptimizationResult } from "../types";

interface StrategyPerformanceProps {
  activeSymbol: string;
}

export const StrategyPerformance: React.FC<StrategyPerformanceProps> = ({ activeSymbol }) => {
  const [metrics, setMetrics] = useState<StrategyMetrics[]>([]);
  const [loading, setLoading] = useState(true);
  const [calibrating, setCalibrating] = useState(false);
  const [wfoResult, setWfoResult] = useState<WfoOptimizationResult | null>(null);

  const loadMetrics = async () => {
    setLoading(true);
    try {
      const data = await api.getStrategyPerformance();
      setMetrics(data);
    } catch (err) {
      console.error("Failed to load strategy performance:", err);
    } finally {
      setLoading(false);
    }
  };

  const runCalibration = async () => {
    setCalibrating(true);
    try {
      const result = await api.runWfoCalibration(activeSymbol);
      setWfoResult(result);
    } catch (err) {
      console.error("Failed to run WFO calibration:", err);
    } finally {
      setCalibrating(false);
    }
  };

  useEffect(() => {
    loadMetrics();
  }, [activeSymbol]);

  // Aggregate totals across all strategies
  const totalInitiated = metrics.reduce((acc, m) => acc + m.total_cycles_initiated, 0);
  const totalCompleted = metrics.reduce((acc, m) => acc + m.total_cycles_completed, 0);
  const totalWinning = metrics.reduce((acc, m) => acc + m.winning_cycles, 0);
  const totalPnlUsd = metrics.reduce((acc, m) => acc + m.total_pnl_usd, 0);
  const overallWinRate = totalCompleted > 0 ? (totalWinning / totalCompleted) * 100 : 0;
  const avgSharpe = metrics.length > 0 ? metrics.reduce((acc, m) => acc + m.sharpe_ratio, 0) / metrics.length : 0;
  const maxMddUsd = metrics.reduce((acc, m) => Math.max(acc, m.max_drawdown_usd), 0);
  const maxMddPct = metrics.reduce((acc, m) => Math.max(acc, m.max_drawdown_pct), 0);
  const avgApr = metrics.length > 0 ? metrics.reduce((acc, m) => acc + m.annualized_apr_pct, 0) / metrics.length : 0;
  const avgApy = metrics.length > 0 ? metrics.reduce((acc, m) => acc + m.annualized_apy_pct, 0) / metrics.length : 0;

  const getStrategyIcon = (id: string) => {
    switch (id) {
      case "futures":
        return <SpreadGapIcon className="w-4 h-4 text-blue-600" />;
      case "funding":
        return <Percent className="w-4 h-4 text-emerald-600" />;
      case "prediction":
        return <Target className="w-4 h-4 text-purple-600" />;
      case "stat_arb":
        return <Sigma className="w-4 h-4 text-indigo-600" />;
      default:
        return <Activity className="w-4 h-4 text-slate-600" />;
    }
  };

  return (
    <div className="space-y-4 pb-12 select-none">
      {/* Top Banner */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-surface-border pb-2.5">
        <div>
          <div className="flex items-center space-x-2">
            <TrendingUp className="w-5 h-5 text-accent" />
            <h2 className="text-lg font-bold font-mono tracking-wider text-slate-900">Portfolio & Strategy Performance</h2>
          </div>
          <p className="text-xs text-slate-500 font-mono mt-0.5">
            Round-trip closed cycle attribution, risk-adjusted returns, and automatic Walk-Forward Calibration
          </p>
        </div>

        <div className="flex items-center space-x-2">
          <button
            onClick={loadMetrics}
            disabled={loading}
            className="px-3 py-1.5 bg-surface border border-surface-border hover:bg-surface-raised text-slate-700 rounded-lg text-xs font-mono font-medium transition shadow-xs flex items-center space-x-1.5 cursor-pointer"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-accent" : ""}`} />
            <span>Refresh Telemetry</span>
          </button>
        </div>
      </div>

      {/* Aggregate Portfolio KPI Cards */}
      <div className="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-6 gap-3">
        {/* Total Realized PnL */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Total Realized PnL</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className={`text-base font-bold font-mono tabular-nums ${totalPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>
              {totalPnlUsd >= 0 ? `+$${totalPnlUsd.toFixed(2)}` : `-$${Math.abs(totalPnlUsd).toFixed(2)}`}
            </span>
          </div>
          <span className="text-xs text-slate-400 font-mono">Net of taker fees & slippage</span>
        </div>

        {/* Win Rate */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Win Rate</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-slate-900">{overallWinRate.toFixed(1)}%</span>
            <span className="text-xs text-slate-400 font-mono tabular-nums">({totalWinning}/{totalCompleted})</span>
          </div>
          <span className="text-xs text-slate-400 font-mono">Profitable Closed Cycles</span>
        </div>

        {/* Closed Cycles */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Cycles Completed</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-slate-900">{totalCompleted}</span>
            <span className="text-xs text-slate-400 font-mono tabular-nums">/ {totalInitiated} initiated</span>
          </div>
          <span className="text-xs text-slate-400 font-mono">100% two-leg atomic fills</span>
        </div>

        {/* Sharpe Ratio */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Sharpe Ratio</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-accent">{avgSharpe.toFixed(2)}</span>
            <span className="text-xs text-slate-400 font-mono">Ann.</span>
          </div>
          <span className="text-xs text-slate-400 font-mono">Risk-adjusted return</span>
        </div>

        {/* Max Drawdown */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Max Drawdown</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-warn">
              ${maxMddUsd.toFixed(2)} ({maxMddPct.toFixed(2)}%)
            </span>
          </div>
          <span className="text-xs text-slate-400 font-mono">Peak-to-trough capital dip</span>
        </div>

        {/* APR / APY */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Est. APR / APY</span>
          <div className="mt-1 flex items-baseline space-x-1">
            <span className="text-base font-bold font-mono tabular-nums text-slate-900">{avgApr.toFixed(0)}%</span>
            <span className="text-xs text-slate-400 font-mono tabular-nums">/ {avgApy.toFixed(0)}%</span>
          </div>
          <span className="text-xs text-slate-400 font-mono">Annualized compounded run-rate</span>
        </div>
      </div>

      {/* Strategy Breakdown Cards */}
      <div className="space-y-3">
        <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 flex items-center space-x-2">
          <BarChart2 className="w-4 h-4 text-accent" />
          <span>Strategy Breakdown & Virtual Lots Ledger</span>
        </h3>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {metrics.map((s) => (
            <div
              key={s.strategy_id}
              className="p-4 bg-surface border border-surface-border rounded-xl shadow-card space-y-3 hover:border-accent-line/80 transition"
            >
              <div className="flex items-start justify-between">
                <div className="flex items-center space-x-2.5">
                  <div className="p-2 rounded-lg bg-surface-raised border border-surface-border">
                    {getStrategyIcon(s.strategy_id)}
                  </div>
                  <div>
                    <h4 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">{formatStrategyName(s.strategy_id)}</h4>
                    <span className="text-xs text-slate-400 font-mono tracking-wider">
                      Strategy ID: {s.strategy_id}
                    </span>
                  </div>
                </div>

                <span className="px-2.5 py-0.5 rounded-full text-xs font-mono font-semibold bg-profit-dim text-profit border border-profit-border">
                  Active
                </span>
              </div>

              <div className="grid grid-cols-4 gap-2 pt-1 border-t border-surface-border/60 text-center font-mono tabular-nums">
                <div className="p-1.5 rounded-md bg-surface-raised/60">
                  <span className="text-xs text-slate-400 block">Win Rate</span>
                  <span className="text-xs font-bold text-slate-900">{s.win_rate_pct.toFixed(1)}%</span>
                </div>
                <div className="p-1.5 rounded-md bg-surface-raised/60">
                  <span className="text-xs text-slate-400 block">Net PnL</span>
                  <span className={`text-xs font-bold ${s.total_pnl_usd >= 0 ? "text-profit" : "text-loss"}`}>
                    +${s.total_pnl_usd.toFixed(1)}
                  </span>
                </div>
                <div className="p-1.5 rounded-md bg-surface-raised/60">
                  <span className="text-xs text-slate-400 block">Avg / Trade</span>
                  <span className="text-xs font-bold text-accent">+{s.avg_pnl_bps.toFixed(1)} BPS</span>
                </div>
                <div className="p-1.5 rounded-md bg-surface-raised/60">
                  <span className="text-xs text-slate-400 block">Sharpe</span>
                  <span className="text-xs font-bold text-slate-900">{s.sharpe_ratio.toFixed(2)}</span>
                </div>
              </div>

              <div className="flex items-center justify-between text-xs font-mono tabular-nums text-slate-500 pt-1">
                <span>Cycles: {s.total_cycles_completed} completed / {s.total_cycles_initiated} tried</span>
                <span>MDD: ${s.max_drawdown_usd.toFixed(1)} ({s.max_drawdown_pct.toFixed(2)}%)</span>
                <span>APR: {s.annualized_apr_pct.toFixed(0)}%</span>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Built-in Algorithmic Calibration Toolkit (WFO) */}
      <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card space-y-3">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 border-b border-surface-border pb-2.5">
          <div className="flex items-center space-x-2.5">
            <div className="p-2 rounded-lg bg-accent-dim border border-accent-line text-accent">
              <Cpu className="w-5 h-5" />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                  Algorithmic Walk-Forward Calibration Toolkit
                </h3>
                <span className="px-2 py-0.5 rounded-full text-xs font-mono font-semibold bg-blue-50 text-accent border border-blue-200">
                  Active for: {activeSymbol}
                </span>
              </div>
              <p className="text-xs text-slate-500 font-mono mt-0.5">
                Evaluates candidate spread estimators (SMA, EMA, WMA, VWMA, SMMA) across quote update windows
              </p>
            </div>
          </div>

          <button
            onClick={runCalibration}
            disabled
            title="Native WFO requires recorded quote history — not yet available"
            className="px-3.5 py-2 bg-accent hover:bg-blue-600 disabled:opacity-50 text-white rounded-lg text-xs font-mono font-medium transition shadow-xs flex items-center space-x-2 shrink-0 cursor-pointer"
          >
            <Sliders className={`w-3.5 h-3.5 ${calibrating ? "animate-spin" : ""}`} />
            <span>
              {calibrating ? "Optimizing..." : "WFO unavailable — recorded quote history required"}
            </span>
          </button>
        </div>

        {wfoResult && (
          <div className="space-y-4 pt-1">
            {/* Recommendation Highlight Box */}
            <div className="p-4 rounded-xl bg-blue-50/70 border border-blue-200 flex flex-col md:flex-row md:items-center justify-between gap-3">
              <div className="flex items-start space-x-3">
                <CheckCircle2 className="w-5 h-5 text-accent shrink-0 mt-0.5" />
                <div>
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-bold font-mono text-slate-900">
                      Optimal Estimator: {wfoResult.optimal_estimator} ({wfoResult.optimal_window_size} Quote Updates)
                    </span>
                    <span className="px-2.5 py-0.5 rounded-full text-xs font-mono font-bold bg-profit-dim text-profit border border-profit-border tabular-nums">
                      OOS Sharpe: {wfoResult.out_of_sample_score.toFixed(2)}
                    </span>
                  </div>
                  <p className="text-xs text-slate-600 mt-1">{wfoResult.recommendation_notes}</p>
                </div>
              </div>

              <div className="flex items-center space-x-3 shrink-0 text-xs font-mono tabular-nums">
                <div className="text-right">
                  <span className="text-xs text-slate-400 block">In-Sample Sharpe</span>
                  <span className="font-bold text-slate-900">{wfoResult.in_sample_score.toFixed(2)}</span>
                </div>
                <div className="text-right">
                  <span className="text-xs text-slate-400 block">Out-of-Sample</span>
                  <span className="font-bold text-accent">{wfoResult.out_of_sample_score.toFixed(2)}</span>
                </div>
              </div>
            </div>

            {/* Candidate Estimators Table */}
            <div className="border border-surface-border rounded-xl overflow-hidden shadow-2xs">
              <div className="bg-surface-raised px-4 py-2.5 border-b border-surface-border">
                <span className="text-xs font-bold font-mono text-slate-800">
                  Walk-Forward Evaluation Matrix (60% Calibration / 40% Validation)
                </span>
              </div>
              <div className="overflow-x-auto">
                <table className="w-full text-xs font-mono tabular-nums text-left">
                  <thead className="bg-surface-raised/60 text-xs text-slate-500 tracking-wider border-b border-surface-border">
                    <tr>
                      <th className="px-4 py-2.5 font-semibold">Estimator Variant</th>
                      <th className="px-3 py-2.5 font-semibold">Window (Ticks)</th>
                      <th className="px-3 py-2.5 font-semibold">In-Sample Sharpe</th>
                      <th className="px-3 py-2.5 font-semibold">OOS Sharpe</th>
                      <th className="px-3 py-2.5 font-semibold">Tracking Err (BPS)</th>
                      <th className="px-3 py-2.5 font-semibold">Sim. Net Profit</th>
                      <th className="px-3 py-2.5 font-semibold">Win Rate</th>
                      <th className="px-3 py-2.5 font-semibold">Trades</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-surface-border">
                    {wfoResult.candidate_evaluations.map((c, idx) => {
                      const isTop =
                        c.estimator === wfoResult.optimal_estimator &&
                        c.window_size === wfoResult.optimal_window_size;
                      return (
                        <tr
                          key={idx}
                          className={`${
                            isTop ? "bg-blue-50/50 font-semibold" : "hover:bg-slate-50/60"
                          } transition`}
                        >
                          <td className="px-4 py-2.5 flex items-center space-x-2">
                            {isTop && <Award className="w-3.5 h-3.5 text-accent" />}
                            <span>
                              {c.estimator === "VWMA"
                                ? "VWMA (Depth-Weighted)"
                                : c.estimator === "SMMA"
                                ? "SMMA (Wilder's MA)"
                                : c.estimator}
                            </span>
                          </td>
                          <td className="px-3 py-2.5">{c.window_size}</td>
                          <td className="px-3 py-2.5">{c.in_sample_sharpe.toFixed(2)}</td>
                          <td className="px-3 py-2.5">
                            <span className={c.out_of_sample_sharpe > 2.0 ? "text-profit font-bold" : ""}>
                              {c.out_of_sample_sharpe.toFixed(2)}
                            </span>
                          </td>
                          <td className="px-3 py-2.5">{c.tracking_error_bps.toFixed(2)}</td>
                          <td className="px-3 py-2.5 text-profit font-bold">+{c.simulated_net_profit_bps.toFixed(1)} BPS</td>
                          <td className="px-3 py-2.5">{c.win_rate_pct.toFixed(1)}%</td>
                          <td className="px-3 py-2.5">{c.total_trades}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
