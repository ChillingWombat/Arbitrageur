import React, { useState, useEffect, useMemo } from "react";
import {
  TrendingUp,
  Activity,
  ShieldCheck,
  Cpu,
  Percent,
  Target,
  Sigma,
  Coins,
  Triangle,
  Sliders,
  CheckCircle2,
} from "lucide-react";
import { SpreadGapIcon } from "./icons";
import { EmptyState } from "./EmptyState";
import { ExchangeVenueGadget } from "./ExchangeVenueGadget";
import { formatStrategyName } from "../types/tickers";
import { api } from "../services/api";
import { computeExchangeStats } from "../services/exchangeStats";
import type {
  ExecutionLogEntry,
  RiskOverviewDto,
  SystemStatusDto,
  StrategyMetrics,
  UtilisationHistory,
  VenueBalanceInfo,
  WfoOptimizationResult,
} from "../types";

interface PortfolioMonitorProps {
  /** Selected ticker; null until the user explicitly picks one in the explorer. */
  activeSymbol: string | null;
  riskData: RiskOverviewDto | null;
  status: SystemStatusDto | null;
  onResetCircuitBreaker: () => void;
  onTriggerKillswitch: () => void;
}

const PortfolioMonitorInner: React.FC<PortfolioMonitorProps> = ({
  activeSymbol,
  riskData,
  status,
  onResetCircuitBreaker,
  onTriggerKillswitch,
}) => {
  const [metrics, setMetrics] = useState<StrategyMetrics[]>([]);
  const [strategyFilter, setStrategyFilter] = useState<string>("all");
  const [calibrating, setCalibrating] = useState(false);
  const [wfoResult, setWfoResult] = useState<WfoOptimizationResult | null>(null);
  // Per-venue inputs for the exchange gadget. Null until the first
  // successful read (never an empty fake).
  const [execLogs, setExecLogs] = useState<ExecutionLogEntry[] | null>(null);
  const [venueBalances, setVenueBalances] = useState<VenueBalanceInfo[] | null>(null);
  // Per-venue 24h utilisation histories keyed by lowercase venue id. Absent
  // until the first successful read — the gadget keeps the "—" fallback.
  const [utilHistories, setUtilHistories] = useState<Record<string, UtilisationHistory | null>>({});

  const loadStrategyMetrics = async () => {
    try {
      const data = await api.getStrategyPerformance();
      setMetrics(data);
    } catch (err) {
      console.error("Failed to load strategy performance:", err);
    }
  };

  const loadExchangeInputs = async () => {
    try {
      const [logs, balances] = await Promise.all([
        api.getExecutionLogs(),
        api.getVenueBalances().catch(() => null),
      ]);
      setExecLogs(logs);
      if (balances) {
        setVenueBalances(balances);
        const venues = [...new Set(balances.map((b) => (b.venue ?? "").trim().toLowerCase()).filter((v) => v !== ""))];
        const settled = await Promise.all(
          venues.map(async (venue) => {
            try {
              const hist = await api.getUtilisationHistory(venue, 24);
              return [venue, hist] as const;
            } catch {
              return [venue, null] as const;
            }
          }),
        );
        setUtilHistories(Object.fromEntries(settled));
      }
    } catch (err) {
      console.error("Failed to load exchange stats inputs:", err);
    }
  };

  const runCalibration = async () => {
    if (!activeSymbol) return;
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

  // Metrics stream automatically — no manual refresh. riskData/status props
  // already update on the app's 3s poll; mirror that cadence here.
  useEffect(() => {
    loadStrategyMetrics();
    loadExchangeInputs();
    const interval = setInterval(() => {
      loadStrategyMetrics();
      loadExchangeInputs();
    }, 3000);
    return () => clearInterval(interval);
  }, [activeSymbol]);

  const exchangeRows = useMemo(
    () =>
      execLogs == null && venueBalances == null
        ? null
        : computeExchangeStats(execLogs, venueBalances, utilHistories),
    [execLogs, venueBalances, utilHistories],
  );

  const isTripped = status?.circuit_breaker_tripped ?? riskData?.is_halted ?? false;
  const isPaused = status?.is_paused ?? false;
  const margin = riskData?.margin_health;

  // Strategies with recorded metrics drive the monitor selector options
  const strategyOptions = useMemo(
    () => [...new Set(metrics.map((m) => m.strategy_id))].sort(),
    [metrics],
  );

  // Metrics scoped to the selected strategy ("all" = aggregate view)
  const visibleMetrics = useMemo(
    () =>
      strategyFilter === "all"
        ? metrics
        : metrics.filter((m) => m.strategy_id === strategyFilter),
    [metrics, strategyFilter],
  );

  // Aggregate strategy metrics (respect the monitor selector)
  const totalCompleted = visibleMetrics.reduce((acc, m) => acc + m.total_cycles_completed, 0);
  const totalWinning = visibleMetrics.reduce((acc, m) => acc + m.winning_cycles, 0);
  const totalPnlUsd = visibleMetrics.reduce((acc, m) => acc + m.total_pnl_usd, 0);
  const overallWinRate = totalCompleted > 0 ? (totalWinning / totalCompleted) * 100 : 0;
  const avgSharpe = visibleMetrics.length > 0 ? visibleMetrics.reduce((acc, m) => acc + m.sharpe_ratio, 0) / visibleMetrics.length : 0;
  const avgApr = visibleMetrics.length > 0 ? visibleMetrics.reduce((acc, m) => acc + m.annualized_apr_pct, 0) / visibleMetrics.length : 0;

  const getStrategyIcon = (id: string) => {
    switch (id) {
      case "spatial_spread":
        return <SpreadGapIcon className="w-4 h-4 text-blue-600" />;
      case "funding_rate":
        return <Percent className="w-4 h-4 text-emerald-600" />;
      case "spot_trade":
      case "spot_arb":
        return <Coins className="w-4 h-4 text-amber-600" />;
      case "triangular_arb":
      case "triangle":
        return <Triangle className="w-4 h-4 text-teal-600" />;
      case "prediction_parity":
        return <Target className="w-4 h-4 text-purple-600" />;
      case "stat_arb":
        return <Sigma className="w-4 h-4 text-indigo-600" />;
      default:
        return <Activity className="w-4 h-4 text-slate-600" />;
    }
  };

  // Strategy display names come from the canonical STRATEGY_DISPLAY_NAMES map
  // (types/tickers) — keyed by the persisted ids: futures/funding/spot/etc.

  return (
    <div className="w-full p-4 space-y-4 overflow-y-auto h-full select-none pb-24">
      {/* 1. Header Control Bar */}
      <div className="bg-surface p-3.5 rounded-lg border border-surface-border flex flex-wrap items-center justify-between gap-3 shadow-2xs">
        <div>
          <div className="flex items-center space-x-2">
            <Activity className="w-4 h-4 text-accent" />
            <h1 className="text-[13px] font-bold font-mono tracking-wider text-slate-900">Portfolio Monitor</h1>
            <span
              className={`px-2.5 py-0.5 rounded-sm text-xs font-mono font-bold ${
                isTripped
                  ? "bg-loss-dim text-loss border border-loss-border"
                  : isPaused
                  ? "bg-warn-dim text-warn border border-warn-border"
                  : "bg-profit-dim text-profit border border-profit-border"
              }`}
            >
              {isTripped ? "Circuit Breaker: Tripped" : isPaused ? "Trading: Paused" : "System Risk: Normal"}
            </span>
          </div>
        </div>

        <div className="flex items-center space-x-3">
          <label className="flex items-center space-x-2 text-xs font-mono text-slate-500">
            <span>Monitor:</span>
            <select
              value={strategyFilter}
              onChange={(e) => setStrategyFilter(e.target.value)}
              className="px-2.5 py-1.5 bg-surface border border-surface-border rounded-md text-xs font-mono font-semibold text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent cursor-pointer"
              title="Select which strategy to monitor"
            >
              <option value="all">All</option>
              {strategyOptions.map((id) => (
                <option key={id} value={id}>
                  {formatStrategyName(id)}
                </option>
              ))}
            </select>
          </label>

          {isTripped ? (
            <button
              onClick={onResetCircuitBreaker}
              className="px-3 py-1.5 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-md text-xs font-mono font-bold flex items-center transition shadow-2xs cursor-pointer"
            >
              <span>Reset Circuit Breaker</span>
            </button>
          ) : (
            <button
              onClick={onTriggerKillswitch}
              className="px-3 py-1.5 bg-loss-dim hover:bg-red-100 text-loss border border-loss-border rounded-md text-xs font-mono font-bold flex items-center transition shadow-2xs cursor-pointer"
            >
              <span>Trigger Manual Trip</span>
            </button>
          )}
        </div>
      </div>

      {/* 2. Top Executive KPI Metric Strip */}
      <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-4">
        {/* Total Equity */}
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
          <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Consolidated Equity</span>
          <div className="text-xl lg:text-2xl font-bold font-mono text-slate-900 tracking-tight">
            ${(riskData?.total_portfolio_value_usd ?? 0).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
          </div>
        </div>

        {/* Realized PnL */}
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
          <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Realized PnL</span>
          <div className={`text-xl lg:text-2xl font-bold font-mono tracking-tight ${totalPnlUsd >= 0 ? "text-profit" : "text-loss"}`}>
            {totalPnlUsd >= 0 ? `+$${totalPnlUsd.toFixed(2)}` : `-$${Math.abs(totalPnlUsd).toFixed(2)}`}
          </div>
        </div>

        {/* Win Rate */}
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
          <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Cycle Win Rate</span>
          <div className="text-xl lg:text-2xl font-bold font-mono text-profit tracking-tight">
            {overallWinRate.toFixed(1)}%
          </div>
        </div>

        {/* Sharpe Ratio */}
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
          <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Sharpe / APR</span>
          <div className="text-xl lg:text-2xl font-bold font-mono text-accent tracking-tight flex items-baseline space-x-1.5">
            <span>{avgSharpe.toFixed(2)}</span>
            <span className="text-xs font-normal text-slate-500 font-mono">({avgApr.toFixed(0)}% APR)</span>
          </div>
        </div>

        {/* Margin Proximity */}
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
          <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Liquidation Buffer</span>
          <div className={`text-xl lg:text-2xl font-bold font-mono tracking-tight ${margin ? "text-profit" : "text-slate-400"}`}>
            {margin ? `+${margin.liquidation_distance_pct.toFixed(1)}%` : "—"}
          </div>
        </div>

        {/* Circuit Breaker Safety */}
        <div className="bg-surface border border-surface-border rounded-lg p-3.5 shadow-2xs space-y-1.5">
          <span className="text-xs font-mono tracking-wider text-slate-500 font-semibold">Tripwire Limit</span>
          <div className="text-xl lg:text-2xl font-bold font-mono text-slate-900 tracking-tight">
            ${riskData?.max_drawdown_limit_usd ?? 1000}
          </div>
          <span className="text-xs font-mono text-slate-400">{riskData?.consecutive_failures ?? 0} / {riskData?.max_failures_allowed ?? 3} err count</span>
        </div>
      </div>

      {/* 3. DeltaGuard & Risk Operations Grid */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-4">
        {/* Left 2 Cols: DeltaGuard Net Position Neutrality Table */}
        <div className="xl:col-span-2 bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-3">
          <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
            <div>
              <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
                <ShieldCheck className="w-5 h-5 text-accent" />
                <span>DeltaGuard Cross-Exchange Signed Net Exposure</span>
              </h2>
              <p className="text-xs text-slate-600 font-mono mt-1">
                Aggregates all open legs across venues to enforce delta-neutrality ($200 USD strict unhedged tolerance).
              </p>
            </div>
            <span className="px-2.5 py-0.5 rounded-sm text-xs font-mono font-bold bg-profit-dim text-profit border border-profit-border">
              Guard Active
            </span>
          </div>

          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs font-mono">
              <thead className="bg-surface-raised">
                <tr className="border-b border-surface-border text-slate-500 text-xs tracking-wider">
                  <th className="py-2 px-2.5">Base Asset</th>
                  <th className="py-2 px-2.5">Net Quantity</th>
                  <th className="py-2 px-2.5">Signed USD Exposure</th>
                  <th className="py-2 px-2.5">Tolerance Limit</th>
                  <th className="py-2 px-2.5 text-right">Status</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border">
                {riskData && riskData.net_deltas.length > 0 ? (
                  riskData.net_deltas.map((delta) => {
                    const isNeutral = Math.abs(delta.net_exposure_usd) <= delta.delta_limit_usd;
                    return (
                      <tr key={delta.asset} className="hover:bg-surface-raised transition">
                        <td className="py-2 px-2.5 font-bold text-sm text-slate-900">{delta.asset}</td>
                        <td className="py-2 px-2.5 text-slate-700">
                          {delta.net_quantity >= 0 ? `+${delta.net_quantity.toFixed(4)}` : delta.net_quantity.toFixed(4)}
                        </td>
                        <td className="py-2 px-2.5">
                          <span
                            className={`font-bold ${
                              Math.abs(delta.net_exposure_usd) > delta.delta_limit_usd ? "text-loss" : "text-slate-900"
                            }`}
                          >
                            ${delta.net_exposure_usd.toFixed(2)}
                          </span>
                        </td>
                        <td className="py-2 px-2.5 text-slate-500">${delta.delta_limit_usd.toLocaleString()} max</td>
                        <td className="py-2 px-2.5 text-right whitespace-nowrap">
                          {isNeutral ? (
                            <span className="px-2.5 py-0.5 rounded-sm text-xs font-bold bg-profit-dim text-profit border border-profit-border whitespace-nowrap inline-block">
                              Delta Neutral
                            </span>
                          ) : (
                            <span className="px-2.5 py-0.5 rounded-sm text-xs font-bold bg-warn-dim text-warn border border-warn-border whitespace-nowrap inline-block">
                              Exposure Skew
                            </span>
                          )}
                        </td>
                      </tr>
                    );
                  })
                ) : (
                  <tr>
                    <td colSpan={5}>
                      <EmptyState
                        title="No Open Positions"
                        hint="Portfolio is 100% flat and delta neutral."
                      />
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </div>

        {/* Right 1 Col: Margin Proximity & Circuit Breaker Tripwires */}
        <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-3">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
            <ShieldCheck className="w-5 h-5 text-emerald-600" />
            <span>Safety Tripwires & Margin Health</span>
          </h2>

          <div className="space-y-3 font-mono text-xs">
            <div className="p-3 bg-surface-raised rounded-md border border-surface-border space-y-2">
              <div className="flex justify-between items-center gap-2">
                <span className="text-slate-500 whitespace-nowrap">Margin Utilization:</span>
                <span className="font-bold text-slate-900 whitespace-nowrap">{margin ? `${margin.margin_ratio_pct.toFixed(1)}%` : "—"}</span>
              </div>
              <div className="h-2.5 w-full bg-slate-200 rounded-full overflow-hidden">
                <div
                  className="h-full bg-accent transition-all duration-300"
                  style={{ width: `${Math.min(100, margin?.margin_ratio_pct ?? 0)}%` }}
                />
              </div>
            </div>

            <div className="p-3 bg-surface-raised rounded-md border border-surface-border space-y-2">
              <div className="flex justify-between items-center gap-2">
                <span className="text-slate-500 whitespace-nowrap">Distance to Liquidation:</span>
                <span className="font-bold text-profit whitespace-nowrap">{margin ? `+${margin.liquidation_distance_pct.toFixed(1)}% safe` : "—"}</span>
              </div>
              <div className="h-2.5 w-full bg-slate-200 rounded-full overflow-hidden">
                <div
                  className="h-full bg-profit transition-all duration-300"
                  style={{ width: `${margin ? Math.min(100, margin.liquidation_distance_pct) : 0}%` }}
                />
              </div>
            </div>

            <div className="p-3 bg-surface-raised rounded-md border border-surface-border space-y-2">
              <div className="flex justify-between items-center gap-2">
                <span className="text-slate-500 whitespace-nowrap">Consecutive Error Buffer:</span>
                <span className="font-bold text-slate-900">{riskData?.consecutive_failures ?? 0} / {riskData?.max_failures_allowed ?? 3}</span>
              </div>
              <div className="h-2.5 w-full bg-slate-200 rounded-full overflow-hidden">
                <div
                  className="h-full bg-amber-500 transition-all duration-300"
                  style={{ width: `${Math.max(5, ((riskData?.consecutive_failures ?? 0) / (riskData?.max_failures_allowed ?? 3)) * 100)}%` }}
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* 3b. Per-Exchange Stats (per venue, existing IPC only) */}
      <ExchangeVenueGadget rows={exchangeRows} />

      {/* 4. Strategy Performance Attribution Breakdown */}
      <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-3">
        <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
          <div>
            <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
              <TrendingUp className="w-5 h-5 text-accent" />
              <span>Multi-Strategy PnL & Cycle Attribution</span>
            </h2>
            <p className="text-xs text-slate-600 font-mono mt-1">
              Closed cycle attribution, win rate, and annualized metrics across active trading strategies.
            </p>
          </div>
          <span className="text-xs font-mono font-bold text-slate-600">
            {visibleMetrics.length} {strategyFilter === "all" ? "Strategies" : "Strategy"} Active
          </span>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4">
          {visibleMetrics.map((m) => {
            const winRate = m.total_cycles_completed > 0 ? (m.winning_cycles / m.total_cycles_completed) * 100 : 0;
            return (
              <div key={m.strategy_id} className="bg-surface-raised/60 border border-surface-border rounded-md p-3 space-y-2">
                <div className="flex items-center space-x-3">
                  <div className="p-2 bg-surface rounded-md border border-surface-border shadow-2xs">
                    {getStrategyIcon(m.strategy_id)}
                  </div>
                  <div className="min-w-0 flex-1">
                    <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900" title={formatStrategyName(m.strategy_id)}>
                      {formatStrategyName(m.strategy_id)}
                    </h3>
                    <span className="text-xs font-mono text-slate-400 capitalize">{m.strategy_id.replace("_", " ")}</span>
                  </div>
                </div>

                <div className="space-y-2 pt-1 text-xs font-mono">
                  <div className="flex justify-between">
                    <span className="text-slate-500">Realized PnL:</span>
                    <span className={`font-bold ${m.total_pnl_usd >= 0 ? "text-profit" : "text-loss"}`}>
                      {m.total_pnl_usd >= 0 ? `+$${m.total_pnl_usd.toFixed(2)}` : `-$${Math.abs(m.total_pnl_usd).toFixed(2)}`}
                    </span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-slate-500">Win Rate:</span>
                    <span className="font-bold text-slate-800">{winRate.toFixed(1)}% ({m.winning_cycles}/{m.total_cycles_completed})</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-slate-500">Sharpe Ratio:</span>
                    <span className="font-bold text-accent">{m.sharpe_ratio.toFixed(2)}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-slate-500">APR:</span>
                    <span className="font-semibold text-slate-700">{m.annualized_apr_pct.toFixed(1)}%</span>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* 5. Walk-Forward Optimization (WFO) Engine Preview */}
      <div className="bg-surface rounded-lg border border-surface-border p-3.5 shadow-2xs space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-surface-border pb-2.5">
          <div>
            <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
              <Cpu className="w-5 h-5 text-purple-600" />
              <span>Walk-Forward Optimization (WFO) Parameter Calibration</span>
            </h2>
            <p className="text-xs text-slate-600 font-mono mt-1">
              Calibrates candidate spread estimators across quote update windows for optimal out-of-sample Sharpe.
            </p>
          </div>

          <button
            onClick={runCalibration}
            disabled
            title="Native WFO requires recorded quote history — not yet available"
            className="px-3 py-1.5 bg-accent hover:bg-blue-600 disabled:bg-slate-400 disabled:hover:bg-slate-400 text-white rounded-md text-xs font-mono font-bold transition shadow-2xs flex items-center space-x-2 cursor-pointer disabled:cursor-not-allowed"
          >
            <Sliders className={`w-4 h-4 ${calibrating ? "animate-spin" : ""}`} />
            <span>
              {calibrating ? "Optimizing..." : "WFO unavailable — recorded quote history required"}
            </span>
          </button>
        </div>

        {wfoResult ? (
          <div className="space-y-3 font-mono text-xs">
            <div className="p-3.5 rounded-md bg-blue-50/80 border border-blue-200 flex flex-col md:flex-row md:items-center justify-between gap-3">
              <div className="flex items-start space-x-3.5">
                <CheckCircle2 className="w-5 h-5 text-accent shrink-0 mt-0.5" />
                <div>
                  <div className="flex items-center space-x-2">
                    <span className="text-sm font-bold font-mono text-slate-900">
                      Optimal Estimator: {wfoResult.optimal_estimator} ({wfoResult.optimal_window_size} Quote Updates)
                    </span>
                    <span className="px-2.5 py-0.5 rounded-sm text-xs font-mono font-bold bg-emerald-100 text-emerald-800 border border-emerald-300">
                      OOS Score: {wfoResult.out_of_sample_score.toFixed(2)}
                    </span>
                  </div>
                  <p className="text-xs text-slate-700 mt-1">{wfoResult.recommendation_notes}</p>
                </div>
              </div>

              <div className="flex items-center space-x-4 shrink-0 text-xs font-mono">
                <div className="text-right">
                  <span className="text-xs text-slate-400 block">In-Sample</span>
                  <span className="font-bold text-slate-900">{wfoResult.in_sample_score.toFixed(2)}</span>
                </div>
                <div className="text-right">
                  <span className="text-xs text-slate-400 block">Out-of-Sample</span>
                  <span className="font-bold text-accent">{wfoResult.out_of_sample_score.toFixed(2)}</span>
                </div>
              </div>
            </div>

            {wfoResult.candidate_evaluations && wfoResult.candidate_evaluations.length > 0 && (
              <div className="border border-surface-border rounded-md overflow-hidden">
                <table className="w-full text-left text-xs font-mono">
                  <thead>
                    <tr className="bg-surface-raised border-b border-surface-border text-slate-500 text-xs tracking-wider">
                      <th className="py-2 px-2.5">Estimator</th>
                      <th className="py-2 px-2.5">Window</th>
                      <th className="py-2 px-2.5">In-Sample Sharpe</th>
                      <th className="py-2 px-2.5">Out-of-Sample Sharpe</th>
                      <th className="py-2 px-2.5">Sim. Net Profit</th>
                      <th className="py-2 px-2.5 text-right">Win Rate</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-surface-border">
                    {wfoResult.candidate_evaluations.map((c, idx) => (
                      <tr key={idx} className="hover:bg-surface-raised transition">
                        <td className="py-2 px-2.5 font-semibold text-slate-900">{c.estimator}</td>
                        <td className="py-2 px-2.5 text-slate-700">{c.window_size} ticks</td>
                        <td className="py-2 px-2.5 text-slate-800">{c.in_sample_sharpe.toFixed(2)}</td>
                        <td className="py-2 px-2.5 font-bold text-accent">{c.out_of_sample_sharpe.toFixed(2)}</td>
                        <td className="py-2 px-2.5 text-profit font-semibold">+{c.simulated_net_profit_bps.toFixed(1)} BPS</td>
                        <td className="py-2 px-2.5 text-right text-slate-800">{c.win_rate_pct.toFixed(1)}%</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        ) : (
          <EmptyState
            title="WFO Unavailable"
            hint="Recorded quote history is required for calibration."
          />
        )}
      </div>
    </div>
  );
};

export const PortfolioMonitor: React.FC<PortfolioMonitorProps> = React.memo(PortfolioMonitorInner);
