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
import { formatCurrencyPnl, formatStrategyName } from "../types/tickers";
import { api } from "../services/api";
import type { CalibrationReportDto, StrategyMetrics, WfoOptimizationResult } from "../types";

interface StrategyPerformanceProps {
  activeSymbol: string;
}

export const StrategyPerformance: React.FC<StrategyPerformanceProps> = ({ activeSymbol }) => {
  const [metrics, setMetrics] = useState<StrategyMetrics[]>([]);
  const [loading, setLoading] = useState(true);
  const [calibrating, setCalibrating] = useState(false);
  const [wfoResult, setWfoResult] = useState<WfoOptimizationResult | null>(null);
  const [wfoError, setWfoError] = useState<string | null>(null);
  const [forecastCal, setForecastCal] = useState<CalibrationReportDto | null>(null);

  const loadMetrics = async () => {
    setLoading(true);
    try {
      const [data, cal] = await Promise.all([
        api.getStrategyPerformance(),
        api.getCalibrationReport().catch(() => null),
      ]);
      setMetrics(data);
      setForecastCal(cal);
    } catch (err) {
      console.error("Failed to load strategy performance:", err);
    } finally {
      setLoading(false);
    }
  };

  const runCalibration = async () => {
    setCalibrating(true);
    setWfoError(null);
    try {
      const result = await api.runWfoCalibration(activeSymbol);
      setWfoResult(result);
    } catch (err) {
      setWfoError(err instanceof Error ? err.message : String(err));
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
        return <Percent className="w-4 h-4 text-emerald-700" />;
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
            <TrendingUp className="w-5 h-5 text-accent-strong" />
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
            className="px-2 py-2 bg-surface border border-surface-border hover:bg-surface-raised text-slate-700 rounded-lg text-xs font-mono font-medium transition shadow-xs flex items-center space-x-1.5 cursor-pointer"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-accent-strong" : ""}`} />
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
            <span className={`text-base font-bold font-mono tabular-nums ${Number.isFinite(totalPnlUsd) ? (totalPnlUsd >= 0 ? "text-profit" : "text-loss") : "text-slate-500"}`}>
              {formatCurrencyPnl(totalPnlUsd)}
            </span>
          </div>
          <span className="text-xs text-muted font-mono">Net of taker fees & slippage</span>
        </div>

        {/* Win Rate */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Win Rate</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-slate-900">{overallWinRate.toFixed(1)}%</span>
            <span className="text-xs text-muted font-mono tabular-nums">({totalWinning}/{totalCompleted})</span>
          </div>
          <span className="text-xs text-muted font-mono">Profitable Closed Cycles</span>
        </div>

        {/* Closed Cycles */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Cycles Completed</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-slate-900">{totalCompleted}</span>
            <span className="text-xs text-muted font-mono tabular-nums">/ {totalInitiated} initiated</span>
          </div>
          <span className="text-xs text-muted font-mono">100% two-leg atomic fills</span>
        </div>

        {/* Sharpe Ratio */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Sharpe Ratio</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-accent-strong">{avgSharpe.toFixed(2)}</span>
            <span className="text-xs text-muted font-mono">Ann.</span>
          </div>
          <span className="text-xs text-muted font-mono">Risk-adjusted return</span>
        </div>

        {/* Max Drawdown */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Max Drawdown</span>
          <div className="mt-1 flex items-baseline space-x-1.5">
            <span className="text-base font-bold font-mono tabular-nums text-warn-strong">
              ${maxMddUsd.toFixed(2)} ({maxMddPct.toFixed(2)}%)
            </span>
          </div>
          <span className="text-xs text-muted font-mono">Peak-to-trough capital dip</span>
        </div>

        {/* APR / APY */}
        <div className="p-4 bg-surface border border-surface-border rounded-xl shadow-card">
          <span className="text-xs font-mono text-slate-500 font-semibold block">Est. APR / APY</span>
          <div className="mt-1 flex items-baseline space-x-1">
            <span className="text-base font-bold font-mono tabular-nums text-slate-900">{avgApr.toFixed(0)}%</span>
            <span className="text-xs text-muted font-mono tabular-nums">/ {avgApy.toFixed(0)}%</span>
          </div>
          <span className="text-xs text-muted font-mono">Annualized compounded run-rate</span>
        </div>
      </div>

      {/* Strategy Breakdown Cards */}
      <div className="space-y-3">
        <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900 flex items-center space-x-2">
          <BarChart2 className="w-4 h-4 text-accent-strong" />
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
                    <span className="text-xs text-muted font-mono tracking-wider">
                      Strategy ID: {s.strategy_id}
                    </span>
                  </div>
                </div>

                <span className="px-2.5 py-0.5 rounded-full text-xs font-mono font-semibold bg-profit-dim text-profit border border-profit-border">
                  Active
                </span>
              </div>

              <div className="grid grid-cols-4 gap-2 pt-1 border-t border-surface-border/60 text-center font-mono tabular-nums">
                <div className="p-1.5 rounded-lg bg-surface-raised/60">
                  <span className="text-xs text-muted block">Win Rate</span>
                  <span className="text-xs font-bold text-slate-900">{s.win_rate_pct.toFixed(1)}%</span>
                </div>
                <div className="p-1.5 rounded-lg bg-surface-raised/60">
                  <span className="text-xs text-muted block">Net PnL</span>
                  <span className={`text-xs font-bold ${s.total_pnl_usd >= 0 ? "text-profit" : "text-loss"}`}>
                    {formatCurrencyPnl(s.total_pnl_usd)}
                  </span>
                </div>
                <div className="p-1.5 rounded-lg bg-surface-raised/60">
                  <span className="text-xs text-muted block">Avg / Trade</span>
                  <span className="text-xs font-bold text-accent-strong">
                    {Number.isFinite(s.avg_pnl_bps) ? `${s.avg_pnl_bps > 0 ? "+" : ""}${s.avg_pnl_bps.toFixed(1)} BPS` : "—"}
                  </span>
                </div>
                <div className="p-1.5 rounded-lg bg-surface-raised/60">
                  <span className="text-xs text-muted block">Sharpe</span>
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
            <div className="p-2 rounded-lg bg-accent-dim border border-accent-line text-accent-strong">
              <Cpu className="w-5 h-5" />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                  Algorithmic Walk-Forward Calibration Toolkit
                </h3>
                <span className="px-2 py-0.5 rounded-full text-xs font-mono font-semibold bg-blue-50 text-accent-strong border border-blue-200">
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
            disabled={calibrating}
            title="Walk-forward optimization over recorded spread history for the active symbol"
            className="px-2 py-2 bg-accent hover:bg-blue-600 disabled:opacity-50 text-white rounded-lg text-xs font-mono font-medium transition shadow-xs flex items-center space-x-2 shrink-0 cursor-pointer"
          >
            <Sliders className={`w-3.5 h-3.5 ${calibrating ? "animate-spin" : ""}`} />
            <span>
              {calibrating ? "Optimizing..." : `Run WFO — ${activeSymbol}`}
            </span>
          </button>
        </div>

        {wfoError && (
          <div className="p-3 rounded-xl bg-loss-dim border border-loss-border text-xs font-mono text-loss">
            {wfoError}
          </div>
        )}

        {wfoResult && (
          <div className="space-y-4 pt-1">
            {/* Recommendation Highlight Box */}
            <div className="p-4 rounded-xl bg-blue-50/70 border border-blue-200 flex flex-col md:flex-row md:items-center justify-between gap-3">
              <div className="flex items-start space-x-3">
                <CheckCircle2 className="w-5 h-5 text-accent-strong shrink-0 mt-0.5" />
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
                  <span className="text-xs text-muted block">In-Sample Sharpe</span>
                  <span className="font-bold text-slate-900">{wfoResult.in_sample_score.toFixed(2)}</span>
                </div>
                <div className="text-right">
                  <span className="text-xs text-muted block">Out-of-Sample</span>
                  <span className="font-bold text-accent-strong">{wfoResult.out_of_sample_score.toFixed(2)}</span>
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
                            {isTop && <Award className="w-3.5 h-3.5 text-accent-strong" />}
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

      {/* Forecast calibration — are emitted probabilities honest? */}
      {forecastCal != null && (forecastCal.scored > 0 || forecastCal.unresolved > 0 || forecastCal.auditOnly > 0) && (
        <div className="space-y-3">
          <div className="flex items-center justify-between gap-3 border-b border-surface-border pb-2.5">
            <div className="flex items-center space-x-2.5">
              <div className="p-2 rounded-lg bg-surface-raised border border-surface-border text-slate-600">
                <Target className="w-5 h-5" />
              </div>
              <div>
                <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">
                  Forecast Calibration
                </h3>
                <p className="text-xs text-slate-500 font-mono mt-0.5">
                  Predicted probabilities vs reconciled outcomes — a probability never scored is a guess
                </p>
              </div>
            </div>
            <div className="flex items-center space-x-4 text-xs font-mono tabular-nums">
              <div className="text-right">
                <span className="text-[10px] text-muted block">Scored</span>
                <span className="font-bold text-slate-900">{forecastCal.scored}</span>
              </div>
              <div className="text-right" title="Predictions waiting on a reconciled close">
                <span className="text-[10px] text-muted block">Pending</span>
                <span className="font-bold text-slate-900">{forecastCal.unresolved}</span>
              </div>
              <div className="text-right" title="Brier score: mean squared forecast error. 0.25 = always-50% baseline; lower is better.">
                <span className="text-[10px] text-muted block">Brier</span>
                <span className="font-bold text-slate-900">
                  {forecastCal.overall.brier != null ? forecastCal.overall.brier.toFixed(3) : "—"}
                </span>
              </div>
              <div className="text-right" title="OLS calibration slope/intercept: ideal = 1.0 / 0.0. Slope < 1 with intercept > 0 = overconfident tails.">
                <span className="text-[10px] text-muted block">Cal. slope / int.</span>
                <span className="font-bold text-slate-900">
                  {forecastCal.overall.slope != null ? forecastCal.overall.slope.toFixed(2) : "—"}
                  {" / "}
                  {forecastCal.overall.intercept != null ? forecastCal.overall.intercept.toFixed(2) : "—"}
                </span>
              </div>
            </div>
          </div>

          {forecastCal.overall.bins.length > 0 && (
            <div className="border border-surface-border rounded-xl overflow-hidden shadow-2xs">
              <div className="bg-surface-raised px-4 py-2 border-b border-surface-border">
                <span className="text-xs font-bold font-mono text-slate-800">
                  Reliability — predicted vs observed win frequency (net-P&L forecasts)
                </span>
              </div>
              <div className="px-4 py-3 flex items-end gap-1.5 h-24">
                {forecastCal.overall.bins.map((b) => (
                  <div
                    key={b.lo}
                    className="flex-1 flex flex-col items-center gap-0.5"
                    title={`p ∈ [${b.lo.toFixed(1)}, ${b.hi.toFixed(1)}): predicted ${(b.meanPred * 100).toFixed(0)}%, observed ${(b.observed * 100).toFixed(0)}% (n=${b.n})`}
                  >
                    <div className="w-full flex flex-col justify-end h-16 relative">
                      <div
                        className="w-full bg-accent/70 rounded-t"
                        style={{ height: `${Math.max(2, b.observed * 100)}%` }}
                      />
                      <div
                        className="absolute w-full border-t-2 border-slate-800/60"
                        style={{ bottom: `${b.meanPred * 100}%` }}
                      />
                    </div>
                    <span className="text-[9px] font-mono text-muted tabular-nums">
                      {b.meanPred.toFixed(1)}
                    </span>
                  </div>
                ))}
              </div>
              <div className="px-4 pb-2 text-[10px] font-mono text-muted">
                Bars = observed win frequency; tick marks = mean predicted probability. Bar below the tick = overconfident.
                {forecastCal.auditOnly > 0 && ` · ${forecastCal.auditOnly} audit-only predictions (reversion/persistence) not scored against P&L.`}
              </div>
            </div>
          )}

          {forecastCal.cuts.length > 1 && (
            <div className="border border-surface-border rounded-xl overflow-hidden shadow-2xs">
              <table className="w-full text-xs font-mono tabular-nums text-left">
                <thead className="bg-surface-raised/60 text-xs text-slate-500 tracking-wider border-b border-surface-border">
                  <tr>
                    <th className="px-4 py-2 font-semibold">Strategy</th>
                    <th className="px-3 py-2 font-semibold">Kind</th>
                    <th className="px-3 py-2 font-semibold">Scored</th>
                    <th className="px-3 py-2 font-semibold">Brier</th>
                    <th className="px-3 py-2 font-semibold">Slope</th>
                    <th className="px-3 py-2 font-semibold">Intercept</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-border">
                  {forecastCal.cuts.map((c) => (
                    <tr key={`${c.strategyId}-${c.kind}`}>
                      <td className="px-4 py-2">{formatStrategyName(c.strategyId)}</td>
                      <td className="px-3 py-2 text-slate-500">{c.kind}</td>
                      <td className="px-3 py-2">{c.report.scored}</td>
                      <td className="px-3 py-2">{c.report.brier != null ? c.report.brier.toFixed(3) : "—"}</td>
                      <td className="px-3 py-2">{c.report.slope != null ? c.report.slope.toFixed(2) : "—"}</td>
                      <td className="px-3 py-2">{c.report.intercept != null ? c.report.intercept.toFixed(2) : "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </div>
  );
};
