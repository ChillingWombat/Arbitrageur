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
  Layers,
  Percent,
  Target,
  FlaskConical,
} from "lucide-react";
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
    // Auto-run initial calibration preview for the active symbol
    runCalibration();
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
      case "spatial_spread":
        return <Layers className="w-4 h-4 text-cyan-400" />;
      case "funding_rate":
        return <Percent className="w-4 h-4 text-emerald-400" />;
      case "prediction_parity":
        return <Target className="w-4 h-4 text-purple-400" />;
      case "stat_arb":
        return <FlaskConical className="w-4 h-4 text-indigo-400" />;
      default:
        return <Activity className="w-4 h-4 text-white/60" />;
    }
  };

  const formatStrategyName = (id: string) => {
    switch (id) {
      case "spatial_spread":
        return "Futures Spread Convergence";
      case "stat_arb":
        return "Statistical Cointegration Spread (OU/Kalman)";
      case "funding_rate":
        return "Perpetual Funding Rate Carry & Basis";
      case "prediction_parity":
        return "Synthetic Outcome Parity (Polymarket / Kalshi)";
      default:
        return id;
    }
  };

  return (
    <div className="p-6 space-y-6 overflow-y-auto h-full max-w-6xl select-none">
      {/* Top Banner */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-white/[0.08] pb-4">
        <div>
          <div className="flex items-center space-x-2.5">
            <div className="w-8 h-8 rounded-lg bg-cyan-500/15 border border-cyan-500/30 flex items-center justify-center text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.25)]">
              <TrendingUp className="w-4 h-4" />
            </div>
            <h2 className="text-lg font-bold font-mono text-white tracking-tight">Portfolio & Strategy Performance</h2>
          </div>
          <p className="text-xs text-white/50 font-mono mt-1">
            Round-trip closed cycle attribution, risk-adjusted returns, and automatic Walk-Forward Calibration
          </p>
        </div>

        <div className="flex items-center space-x-2">
          <button
            onClick={loadMetrics}
            disabled={loading}
            className="px-3.5 py-1.5 glass-panel bg-white/[0.04] border border-white/[0.08] hover:bg-white/[0.08] text-white/90 rounded-xl text-xs font-mono font-medium transition shadow-xs flex items-center space-x-1.5"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-cyan-400" : ""}`} />
            <span>Refresh Telemetry</span>
          </button>
        </div>
      </div>

      {/* Aggregate Portfolio KPI Cards */}
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
        {/* Total Realized PnL */}
        <div className="p-3.5 glass-panel border border-white/[0.08] rounded-xl shadow-xs">
          <span className="text-[10px] uppercase font-mono text-white/40 font-semibold block">Total Realized PnL</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className={`text-base font-bold font-mono tabular-nums ${totalPnlUsd >= 0 ? "text-emerald-400" : "text-rose-400"}`}>
              {totalPnlUsd >= 0 ? `+$${totalPnlUsd.toFixed(2)}` : `-$${Math.abs(totalPnlUsd).toFixed(2)}`}
            </span>
          </div>
          <span className="text-[10px] text-white/40 font-mono">Net of taker fees & slippage</span>
        </div>

        {/* Win Rate */}
        <div className="p-3.5 glass-panel border border-white/[0.08] rounded-xl shadow-xs">
          <span className="text-[10px] uppercase font-mono text-white/40 font-semibold block">Win Rate</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-white">{overallWinRate.toFixed(1)}%</span>
            <span className="text-[10px] text-white/40 font-mono">({totalWinning}/{totalCompleted})</span>
          </div>
          <span className="text-[10px] text-white/40 font-mono">Profitable closed cycles</span>
        </div>

        {/* Closed Cycles */}
        <div className="p-3.5 glass-panel border border-white/[0.08] rounded-xl shadow-xs">
          <span className="text-[10px] uppercase font-mono text-white/40 font-semibold block">Cycles Completed</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-white">{totalCompleted}</span>
            <span className="text-[10px] text-white/40 font-mono">/ {totalInitiated} initiated</span>
          </div>
          <span className="text-[10px] text-white/40 font-mono">100% two-leg atomic fills</span>
        </div>

        {/* Sharpe Ratio */}
        <div className="p-3.5 glass-panel border border-white/[0.08] rounded-xl shadow-xs">
          <span className="text-[10px] uppercase font-mono text-white/40 font-semibold block">Sharpe Ratio</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-cyan-300">{avgSharpe.toFixed(2)}</span>
            <span className="text-[10px] text-white/40 font-mono">Ann.</span>
          </div>
          <span className="text-[10px] text-white/40 font-mono">Risk-adjusted return</span>
        </div>

        {/* Max Drawdown */}
        <div className="p-3.5 glass-panel border border-white/[0.08] rounded-xl shadow-xs">
          <span className="text-[10px] uppercase font-mono text-white/40 font-semibold block">Max Drawdown</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-amber-300">
              ${maxMddUsd.toFixed(2)} ({maxMddPct.toFixed(2)}%)
            </span>
          </div>
          <span className="text-[10px] text-white/40 font-mono">Peak-to-trough capital dip</span>
        </div>

        {/* APR / APY */}
        <div className="p-3.5 glass-panel border border-white/[0.08] rounded-xl shadow-xs">
          <span className="text-[10px] uppercase font-mono text-white/40 font-semibold block">Est. APR / APY</span>
          <div className="mt-1 flex items-baseline space-x-1">
            <span className="text-base font-bold font-mono tabular-nums text-white">{avgApr.toFixed(0)}%</span>
            <span className="text-[10px] text-white/40 font-mono">/ {avgApy.toFixed(0)}%</span>
          </div>
          <span className="text-[10px] text-white/40 font-mono">Annualized compounded run-rate</span>
        </div>
      </div>

      {/* Strategy Breakdown Cards */}
      <div className="space-y-3">
        <h3 className="text-sm font-bold font-mono text-white flex items-center space-x-2">
          <BarChart2 className="w-4 h-4 text-cyan-400" />
          <span>Strategy Breakdown & Virtual Lots Ledger</span>
        </h3>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {metrics.map((s) => (
            <div
              key={s.strategy_id}
              className="p-4 glass-panel border border-white/[0.08] hover:border-cyan-500/40 rounded-2xl shadow-xs space-y-3 transition duration-200"
            >
              <div className="flex items-start justify-between">
                <div className="flex items-center space-x-2.5">
                  <div className="p-2 rounded-xl bg-white/[0.04] border border-white/[0.08]">
                    {getStrategyIcon(s.strategy_id)}
                  </div>
                  <div>
                    <h4 className="text-xs font-bold font-mono text-white">{formatStrategyName(s.strategy_id)}</h4>
                    <span className="text-[10px] text-white/40 font-mono uppercase tracking-wider">
                      Strategy ID: {s.strategy_id}
                    </span>
                  </div>
                </div>

                <span className="chip chip-pos text-[10px] font-mono">
                  ACTIVE
                </span>
              </div>

              <div className="grid grid-cols-4 gap-2 pt-1 border-t border-white/[0.06] text-center font-mono">
                <div className="p-2 rounded-xl bg-white/[0.03] border border-white/[0.04]">
                  <span className="text-[9px] uppercase text-white/40 block">Win Rate</span>
                  <span className="text-xs font-bold text-white tabular-nums">{s.win_rate_pct.toFixed(1)}%</span>
                </div>
                <div className="p-2 rounded-xl bg-white/[0.03] border border-white/[0.04]">
                  <span className="text-[9px] uppercase text-white/40 block">Net PnL</span>
                  <span className={`text-xs font-bold tabular-nums ${s.total_pnl_usd >= 0 ? "text-emerald-400" : "text-rose-400"}`}>
                    +${s.total_pnl_usd.toFixed(1)}
                  </span>
                </div>
                <div className="p-2 rounded-xl bg-white/[0.03] border border-white/[0.04]">
                  <span className="text-[9px] uppercase text-white/40 block">Avg / Trade</span>
                  <span className="text-xs font-bold text-cyan-300 tabular-nums">+{s.avg_pnl_bps.toFixed(1)} BPS</span>
                </div>
                <div className="p-2 rounded-xl bg-white/[0.03] border border-white/[0.04]">
                  <span className="text-[9px] uppercase text-white/40 block">Sharpe</span>
                  <span className="text-xs font-bold text-white tabular-nums">{s.sharpe_ratio.toFixed(2)}</span>
                </div>
              </div>

              <div className="flex items-center justify-between text-[11px] font-mono text-white/50 pt-1 tabular-nums">
                <span>Cycles: {s.total_cycles_completed} completed / {s.total_cycles_initiated} tried</span>
                <span>MDD: ${s.max_drawdown_usd.toFixed(1)} ({s.max_drawdown_pct.toFixed(2)}%)</span>
                <span>APR: {s.annualized_apr_pct.toFixed(0)}%</span>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Built-in Algorithmic Calibration Toolkit (WFO) */}
      <div className="p-5 glass-panel border border-white/[0.08] rounded-2xl shadow-xs space-y-4">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 border-b border-white/[0.08] pb-3">
          <div className="flex items-center space-x-2.5">
            <div className="p-2 rounded-xl bg-cyan-500/15 border border-cyan-500/30 text-cyan-300 shadow-[0_0_12px_rgba(34,211,238,0.2)]">
              <Cpu className="w-5 h-5" />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h3 className="text-sm font-bold font-mono text-white">
                  Algorithmic Walk-Forward Calibration Toolkit
                </h3>
                <span className="chip chip-info text-[10px]">
                  Active for: {activeSymbol}
                </span>
              </div>
              <p className="text-xs text-white/50 font-mono mt-0.5">
                Evaluates candidate spread estimators (SMA, EMA, WMA, VWMA, SMMA) across quote update windows
              </p>
            </div>
          </div>

          <button
            onClick={runCalibration}
            disabled={calibrating}
            className="px-4 py-2 bg-gradient-to-r from-cyan-500 to-indigo-600 hover:from-cyan-400 hover:to-indigo-500 disabled:opacity-50 text-white rounded-xl text-xs font-mono font-medium transition shadow-[0_0_16px_rgba(34,211,238,0.25)] flex items-center space-x-2 shrink-0"
          >
            <Sliders className={`w-3.5 h-3.5 ${calibrating ? "animate-spin" : ""}`} />
            <span>{calibrating ? "Optimizing..." : `Run WFO for ${activeSymbol}`}</span>
          </button>
        </div>

        {wfoResult && (
          <div className="space-y-4 pt-1">
            {/* Recommendation Highlight Box */}
            <div className="p-4 rounded-xl glass-panel bg-gradient-to-r from-cyan-500/10 via-indigo-500/10 to-transparent border border-cyan-500/25 flex flex-col md:flex-row md:items-center justify-between gap-3 shadow-[0_0_20px_rgba(34,211,238,0.06)]">
              <div className="flex items-start space-x-3">
                <CheckCircle2 className="w-5 h-5 text-cyan-300 shrink-0 mt-0.5" />
                <div>
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-bold font-mono text-cyan-300">
                      Optimal Estimator: {wfoResult.optimal_estimator} ({wfoResult.optimal_window_size} Quote Updates)
                    </span>
                    <span className="chip chip-pos text-[10px] font-bold tabular-nums">
                      OOS Sharpe: {wfoResult.out_of_sample_score.toFixed(2)}
                    </span>
                  </div>
                  <p className="text-xs text-white/70 mt-1 leading-relaxed">{wfoResult.recommendation_notes}</p>
                </div>
              </div>

              <div className="flex items-center space-x-4 shrink-0 text-xs font-mono">
                <div className="text-right">
                  <span className="text-[10px] text-white/40 block uppercase">In-Sample Sharpe</span>
                  <span className="font-bold text-white tabular-nums">{wfoResult.in_sample_score.toFixed(2)}</span>
                </div>
                <div className="text-right">
                  <span className="text-[10px] text-white/40 block uppercase">Out-of-Sample</span>
                  <span className="font-bold text-cyan-300 tabular-nums">{wfoResult.out_of_sample_score.toFixed(2)}</span>
                </div>
              </div>
            </div>

            {/* Candidate Estimators Table */}
            <div className="border border-white/[0.08] rounded-xl overflow-hidden glass-panel">
              <div className="bg-white/[0.04] px-4 py-2.5 border-b border-white/[0.08]">
                <span className="text-xs font-bold font-mono text-white/90">
                  Walk-Forward Evaluation Matrix (60% Calibration / 40% Validation)
                </span>
              </div>
              <div className="overflow-x-auto">
                <table className="w-full text-xs font-mono text-left">
                  <thead className="bg-white/[0.02] text-white/40 border-b border-white/[0.06] text-[10px] uppercase tracking-wider">
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
                  <tbody className="divide-y divide-white/[0.06]">
                    {wfoResult.candidate_evaluations.map((c, idx) => {
                      const isTop =
                        c.estimator === wfoResult.optimal_estimator &&
                        c.window_size === wfoResult.optimal_window_size;
                      return (
                        <tr
                          key={idx}
                          className={`${
                            isTop
                              ? "bg-cyan-500/10 border-l-2 border-cyan-400 font-semibold text-white"
                              : "hover:bg-white/[0.03] text-white/80"
                          } transition tabular-nums`}
                        >
                          <td className="px-4 py-2.5 flex items-center space-x-2">
                            {isTop && <Award className="w-3.5 h-3.5 text-cyan-400" />}
                            <span className={isTop ? "text-cyan-300 font-bold" : "text-white"}>
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
                            <span className={c.out_of_sample_sharpe > 2.0 ? "text-emerald-400 font-bold" : ""}>
                              {c.out_of_sample_sharpe.toFixed(2)}
                            </span>
                          </td>
                          <td className="px-3 py-2.5">{c.tracking_error_bps.toFixed(2)}</td>
                          <td className="px-3 py-2.5 text-emerald-400 font-bold">+{c.simulated_net_profit_bps.toFixed(1)} BPS</td>
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
