import React, { useState, useEffect, useMemo } from "react";
import {
  TrendingUp,
  Activity,
  ShieldCheck,
  AlertTriangle,
  Wallet,
  Percent,
  Target,
  Sigma,
  Coins,
  Triangle,
  Ban,
  Eye,
} from "lucide-react";
import { SpreadGapIcon } from "./icons";
import { EmptyState } from "./EmptyState";
import { ExchangeVenueGadget } from "./ExchangeVenueGadget";
import { formatStrategyName } from "../types/tickers";
import { api } from "../services/api";
import { computeExchangeStats } from "../services/exchangeStats";
import type {
  ExecutionLogEntry,
  OpenPositionDto,
  RiskOverviewDto,
  SystemStatusDto,
  StrategyMetrics,
  UtilisationHistory,
  VenueBalanceInfo,
} from "../types";

interface PortfolioMonitorProps {
  riskData: RiskOverviewDto | null;
  status: SystemStatusDto | null;
  /** Global open positions (App fan-out) — drives unrealized PnL + allocation. */
  openPositions?: OpenPositionDto[];
  onResetCircuitBreaker: () => void;
  onTriggerKillswitch: () => void;
}

interface AttentionItem {
  severity: "critical" | "warn";
  title: string;
  detail: string;
}

const usd = (v: number, digits = 2) =>
  `${v >= 0 ? "+" : "-"}$${Math.abs(v).toFixed(digits)}`;
const usdPlain = (v: number, digits = 2) =>
  `$${v.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;

const PortfolioMonitorInner: React.FC<PortfolioMonitorProps> = ({
  riskData,
  status,
  openPositions = [],
  onResetCircuitBreaker,
  onTriggerKillswitch,
}) => {
  const [metrics, setMetrics] = useState<StrategyMetrics[]>([]);
  const [strategyFilter, setStrategyFilter] = useState<string>("all");
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
  }, []);

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

  // --- Derived portfolio truth -------------------------------------------------
  const equity = riskData?.total_portfolio_value_usd ?? 0;
  const markedPositions = useMemo(
    () => openPositions.filter((p) => p.unrealizedPnlUsd != null),
    [openPositions]
  );
  const unrealizedUsd = useMemo(
    () => markedPositions.reduce((acc, p) => acc + (p.unrealizedPnlUsd ?? 0), 0),
    [markedPositions]
  );
  const deployedUsd = useMemo(
    () =>
      openPositions.reduce((acc, p) => acc + (p.notionalUsd != null && p.notionalUsd > 0 ? p.notionalUsd : 0), 0),
    [openPositions]
  );
  const idleUsd = Math.max(0, equity - deployedUsd);

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
  const totalPnlAll = totalPnlUsd + unrealizedUsd;

  // Open positions grouped by strategy for the attribution cards.
  const openByStrategy = useMemo(() => {
    const map = new Map<string, { count: number; unrealized: number }>();
    for (const p of openPositions) {
      const key = (p.strategyId ?? "unknown").toLowerCase();
      const slot = map.get(key) ?? { count: 0, unrealized: 0 };
      slot.count += 1;
      if (p.unrealizedPnlUsd != null) slot.unrealized += p.unrealizedPnlUsd;
      map.set(key, slot);
    }
    return map;
  }, [openPositions]);

  // --- Attention items: what needs the user right now ----------------------------
  const attention: AttentionItem[] = useMemo(() => {
    const items: AttentionItem[] = [];
    if (isTripped)
      items.push({
        severity: "critical",
        title: "Circuit breaker tripped — trading halted",
        detail: status?.circuit_breaker_status || riskData?.circuit_breaker_state || "Manual halt engaged.",
      });
    if (margin != null) {
      if (margin.margin_ratio_pct >= 90)
        items.push({ severity: "critical", title: `Margin utilization ${margin.margin_ratio_pct.toFixed(0)}%`, detail: "Above 90% — reduce exposure or add margin before the next volatility spike." });
      else if (margin.margin_ratio_pct >= 70)
        items.push({ severity: "warn", title: `Margin utilization ${margin.margin_ratio_pct.toFixed(0)}%`, detail: "Climbing — watch new entries sizing." });
      if (margin.liquidation_distance_pct <= 10)
        items.push({ severity: "critical", title: `Liquidation buffer ${margin.liquidation_distance_pct.toFixed(1)}%`, detail: "Dangerously thin — trim or top up immediately." });
      else if (margin.liquidation_distance_pct <= 25)
        items.push({ severity: "warn", title: `Liquidation buffer ${margin.liquidation_distance_pct.toFixed(1)}%`, detail: "Narrowing — avoid adding leverage." });
    }
    const skewed = (riskData?.net_deltas ?? []).filter((d) => Math.abs(d.net_exposure_usd) > d.delta_limit_usd);
    for (const d of skewed.slice(0, 3))
      items.push({ severity: "warn", title: `${d.asset} exposure skew ${usd(d.net_exposure_usd)}`, detail: `Beyond the $${d.delta_limit_usd.toLocaleString()} delta-neutral tolerance.` });
    if (skewed.length > 3)
      items.push({ severity: "warn", title: `${skewed.length - 3} more skewed assets`, detail: "See the DeltaGuard table below." });
    const fails = riskData?.consecutive_failures ?? 0;
    const allowed = riskData?.max_failures_allowed ?? 3;
    if (allowed > 0 && fails >= allowed - 1 && fails > 0)
      items.push({ severity: "warn", title: `Error budget nearly exhausted (${fails}/${allowed})`, detail: "One more consecutive failure trips the breaker." });
    for (const b of venueBalances ?? []) {
      if (b.status === "DEPLETED")
        items.push({ severity: "critical", title: `${b.venue} balance depleted`, detail: b.recommended_action ?? "Top up or disable this venue." });
      else if (b.status === "LOW_MARGIN")
        items.push({ severity: "warn", title: `${b.venue} low margin`, detail: b.recommended_action ?? "Reduce sizing on this venue." });
    }
    return items;
  }, [isTripped, status, riskData, margin, venueBalances]);

  const verdict = isTripped
    ? { label: "HALTED", cls: "bg-loss-dim text-loss border-loss-border" }
    : isPaused
      ? { label: "PAUSED", cls: "bg-warn-dim text-warn border-warn-border" }
      : attention.length > 0
        ? { label: "ATTENTION", cls: "bg-warn-dim text-warn border-warn-border" }
        : { label: "HEALTHY", cls: "bg-profit-dim text-profit border-profit-border" };

  const venueTotal = (venueBalances ?? []).reduce((acc, b) => acc + (b.equity_usd > 0 ? b.equity_usd : 0), 0);

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
    <div className="w-full p-4 pr-5 space-y-4 overflow-y-auto h-full select-none pb-24">
      {/* 1. Portfolio health header: verdict + equity + monitor scope */}
      <div className="bg-surface p-4 rounded-xl border border-surface-border flex flex-wrap items-center justify-between gap-3 shadow-card">
        <div className="flex items-center gap-4 min-w-0">
          <span className={`px-2.5 py-1 rounded-lg text-xs font-mono font-bold border tracking-wider ${verdict.cls}`}>
            {verdict.label}
          </span>
          <div className="min-w-0">
            <div className="flex items-baseline gap-2">
              <span className="text-xl font-bold font-mono text-slate-900 tracking-tight tabular-nums">
                {usdPlain(equity)}
              </span>
              <span className={`text-sm font-mono font-bold tabular-nums ${totalPnlAll >= 0 ? "text-profit" : "text-loss"}`}>
                {usd(totalPnlAll)} total
              </span>
            </div>
            <div className="text-[11px] font-mono text-slate-500 tabular-nums">
              {usd(totalPnlUsd)} realized
              {markedPositions.length > 0 ? ` · ${usd(unrealizedUsd)} unrealized (${markedPositions.length} marked)` : " · flat (no open marks)"}
              {isPaused ? " · engine paused — positions preserved" : ""}
            </div>
          </div>
        </div>

        <label className="flex items-center space-x-2 text-xs font-mono text-slate-500">
          <span>Monitor:</span>
          <select
            value={strategyFilter}
            onChange={(e) => setStrategyFilter(e.target.value)}
            className="px-2.5 py-1.5 bg-surface border border-surface-border rounded-lg text-xs font-mono font-semibold text-slate-800 focus:outline-none focus:ring-1 focus:ring-accent cursor-pointer"
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
      </div>

      {/* 2. Capital: where the money sits + deployed vs idle */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-4">
        <div className="xl:col-span-2 bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
            <Wallet className="w-5 h-5 text-accent" />
            <span>Capital Allocation by Venue</span>
          </h2>
          {(venueBalances ?? []).length > 0 ? (
            <div className="space-y-2.5">
              {venueBalances!.map((b) => {
                const share = venueTotal > 0 && b.equity_usd > 0 ? (b.equity_usd / venueTotal) * 100 : 0;
                const barCls =
                  b.status === "DEPLETED" ? "bg-loss" : b.status === "LOW_MARGIN" ? "bg-amber-500" : "bg-accent";
                return (
                  <div key={`${b.venue}-${b.account_id ?? "default"}`} className="space-y-1">
                    <div className="flex items-center justify-between gap-2 text-xs font-mono">
                      <span className="font-bold text-slate-900 truncate">{b.venue}</span>
                      <span className="tabular-nums text-slate-700">
                        {usdPlain(Math.max(0, b.equity_usd))} <span className="text-slate-400">({share.toFixed(0)}%)</span>
                        {b.available_margin_usd > 0 && (
                          <span className="text-slate-400"> · {usdPlain(b.available_margin_usd)} free</span>
                        )}
                      </span>
                    </div>
                    <div className="h-2 w-full bg-slate-200 rounded-full overflow-hidden">
                      <div className={`h-full ${barCls} transition-all duration-300`} style={{ width: `${Math.min(100, share)}%` }} />
                    </div>
                    {b.status !== "HEALTHY" && b.recommended_action && (
                      <div className="text-[11px] font-mono text-warn">{b.recommended_action}</div>
                    )}
                  </div>
                );
              })}
            </div>
          ) : (
            <EmptyState title="No Venue Balances" hint="Connect an exchange account to see capital allocation." />
          )}
        </div>

        <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
            <TrendingUp className="w-5 h-5 text-accent" />
            <span>Deployed vs Idle</span>
          </h2>
          <div className="space-y-1">
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Deployed in positions</span>
              <span className="font-bold text-slate-900 tabular-nums">{usdPlain(deployedUsd)}</span>
            </div>
            <div className="h-2.5 w-full bg-slate-200 rounded-full overflow-hidden flex">
              <div className="h-full bg-accent transition-all duration-300" style={{ width: `${equity > 0 ? Math.min(100, (deployedUsd / equity) * 100) : 0}%` }} />
            </div>
            <div className="flex justify-between text-xs font-mono">
              <span className="text-slate-500">Idle capital</span>
              <span className="font-bold text-slate-900 tabular-nums">{usdPlain(idleUsd)}</span>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-1.5 font-mono text-[11px] pt-1">
            <div className="px-1.5 py-1 bg-surface-raised rounded-lg border border-surface-border">
              <div className="text-slate-500">Open positions</div>
              <div className="font-bold text-slate-900 tabular-nums text-sm">{openPositions.length}</div>
            </div>
            <div className="px-1.5 py-1 bg-surface-raised rounded-lg border border-surface-border">
              <div className="text-slate-500">Win rate</div>
              <div className="font-bold text-profit tabular-nums text-sm">{overallWinRate.toFixed(0)}%</div>
            </div>
          </div>
        </div>
      </div>

      {/* 3. Attention: what needs the user, worst first */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
        <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
          <Eye className="w-5 h-5 text-accent" />
          <span>Needs Attention</span>
          {attention.length > 0 && (
            <span className="px-1.5 py-0.5 rounded-md text-[11px] font-mono font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60">
              {attention.length}
            </span>
          )}
        </h2>
        {attention.length > 0 ? (
          <div className="space-y-2">
            {attention.map((a, i) => (
              <div
                key={i}
                className={`flex items-start gap-2.5 px-3 py-2.5 rounded-lg border text-xs font-mono ${
                  a.severity === "critical" ? "bg-loss-dim/40 border-loss-border" : "bg-warn-dim/40 border-warn-border"
                }`}
              >
                <AlertTriangle className={`w-4 h-4 shrink-0 mt-0.5 ${a.severity === "critical" ? "text-loss" : "text-warn"}`} />
                <div className="min-w-0">
                  <div className="font-bold text-slate-900">{a.title}</div>
                  <div className="text-slate-600 mt-0.5">{a.detail}</div>
                </div>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState plain title="All clear — nothing needs you right now." />
        )}
      </div>

      {/* 4. Risk detail: DeltaGuard neutrality + margin + breaker controls */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-4">
        {/* Left 2 Cols: DeltaGuard Net Position Neutrality Table */}
        <div className="xl:col-span-2 bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
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
          </div>

          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs font-mono tabular-nums">
              <thead className="bg-surface-raised/60">
                <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
                  <th className="py-2.5 px-3">Base Asset</th>
                  <th className="py-2.5 px-3">Net Quantity</th>
                  <th className="py-2.5 px-3">Signed USD Exposure</th>
                  <th className="py-2.5 px-3">Tolerance Limit</th>
                  <th className="py-2.5 px-3 text-right">Status</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border">
                {riskData && riskData.net_deltas.length > 0 ? (
                  riskData.net_deltas.map((delta) => {
                    const isNeutral = Math.abs(delta.net_exposure_usd) <= delta.delta_limit_usd;
                    return (
                      <tr key={delta.asset} className="hover:bg-surface-raised/40 transition">
                        <td className="py-2.5 px-3 font-bold text-sm text-slate-900">{delta.asset}</td>
                        <td className="py-2.5 px-3 text-slate-700">
                          {delta.net_quantity >= 0 ? `+${delta.net_quantity.toFixed(4)}` : delta.net_quantity.toFixed(4)}
                        </td>
                        <td className="py-2.5 px-3">
                          <span
                            className={`font-bold ${
                              Math.abs(delta.net_exposure_usd) > delta.delta_limit_usd ? "text-loss" : "text-slate-900"
                            }`}
                          >
                            ${delta.net_exposure_usd.toFixed(2)}
                          </span>
                        </td>
                        <td className="py-2.5 px-3 text-slate-500">${delta.delta_limit_usd.toLocaleString()} max</td>
                        <td className="py-2.5 px-3 text-right whitespace-nowrap">
                          {isNeutral ? (
                            <span className="px-2.5 py-0.5 rounded-full text-xs font-bold bg-profit-dim text-profit border border-profit-border whitespace-nowrap inline-block">
                              Delta Neutral
                            </span>
                          ) : (
                            <span className="px-2.5 py-0.5 rounded-full text-xs font-bold bg-warn-dim text-warn border border-warn-border whitespace-nowrap inline-block">
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
        <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
            <ShieldCheck className="w-5 h-5 text-profit" />
            <span>Safety Tripwires & Margin Health</span>
          </h2>

          <div className="space-y-3 font-mono text-xs">
            <div className="p-3 bg-surface-raised rounded-lg border border-surface-border space-y-2">
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

            <div className="p-3 bg-surface-raised rounded-lg border border-surface-border space-y-2">
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

            <div className="p-3 bg-surface-raised rounded-lg border border-surface-border space-y-2">
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
              <div className="flex justify-between items-center gap-2 pt-1">
                <span className="text-slate-500 whitespace-nowrap">Drawdown:</span>
                <span className="font-bold text-slate-900 whitespace-nowrap tabular-nums">
                  {usd(riskData?.current_drawdown_usd ?? 0)} / {usdPlain(riskData?.max_drawdown_limit_usd ?? 1000, 0)} limit
                </span>
              </div>
            </div>

            {isTripped ? (
              <button
                onClick={onResetCircuitBreaker}
                className="w-full px-2 py-2 bg-warn-dim hover:bg-amber-100 text-warn border border-warn-border rounded-lg text-xs font-mono font-bold flex items-center justify-center transition shadow-xs cursor-pointer"
              >
                <Ban className="w-3.5 h-3.5 mr-1.5" />
                <span>Reset Circuit Breaker</span>
              </button>
            ) : (
              <button
                onClick={onTriggerKillswitch}
                className="btn-danger w-full px-2 py-2 text-xs font-mono font-bold flex items-center justify-center shadow-xs cursor-pointer"
                title="Emergency halt — immediately stops all entries"
              >
                <Ban className="w-3.5 h-3.5 mr-1.5" />
                <span>Trigger Manual Trip</span>
              </button>
            )}
          </div>
        </div>
      </div>

      {/* 5. Per-Exchange Stats (per venue, existing IPC only) */}
      <ExchangeVenueGadget rows={exchangeRows} />

      {/* 6. Strategy Performance Attribution Breakdown */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
        <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
          <div>
            <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
              <TrendingUp className="w-5 h-5 text-accent" />
              <span>Multi-Strategy PnL & Cycle Attribution</span>
            </h2>
            <p className="text-xs text-slate-600 font-mono mt-1">
              Closed cycle attribution, live open positions, and annualized metrics across active trading strategies.
            </p>
          </div>
          <span className="text-xs font-mono font-bold text-slate-600">
            {visibleMetrics.length} {strategyFilter === "all" ? "Strategies" : "Strategy"} Active
          </span>
        </div>

        {visibleMetrics.length > 0 ? (
          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4">
            {visibleMetrics.map((m) => {
              const winRate = m.total_cycles_completed > 0 ? (m.winning_cycles / m.total_cycles_completed) * 100 : 0;
              const open = openByStrategy.get(m.strategy_id.toLowerCase()) ?? openByStrategy.get(m.strategy_id);
              return (
                <div key={m.strategy_id} className="bg-surface-raised/60 border border-surface-border rounded-xl p-3.5 space-y-2.5 shadow-2xs">
                  <div className="flex items-center space-x-3">
                    <div className="p-2 bg-surface rounded-lg border border-surface-border shadow-xs">
                      {getStrategyIcon(m.strategy_id)}
                    </div>
                    <div className="min-w-0 flex-1">
                      <h3 className="text-[13px] font-bold font-mono tracking-wider text-slate-900" title={formatStrategyName(m.strategy_id)}>
                        {formatStrategyName(m.strategy_id)}
                      </h3>
                      <span className="text-xs font-mono text-slate-400 capitalize">{m.strategy_id.replace("_", " ")}</span>
                    </div>
                    {open && open.count > 0 && (
                      <span
                        className="px-1.5 py-0.5 rounded-md text-[11px] font-mono font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent border border-accent-line/60"
                        title={`${open.count} open position(s)`}
                      >
                        {open.count} open
                      </span>
                    )}
                  </div>

                  <div className="space-y-2 pt-1 text-xs font-mono tabular-nums">
                    <div className="flex justify-between">
                      <span className="text-slate-500">Realized PnL:</span>
                      <span className={`font-bold ${m.total_pnl_usd >= 0 ? "text-profit" : "text-loss"}`}>
                        {m.total_pnl_usd >= 0 ? `+$${m.total_pnl_usd.toFixed(2)}` : `-$${Math.abs(m.total_pnl_usd).toFixed(2)}`}
                      </span>
                    </div>
                    {open && open.count > 0 && (
                      <div className="flex justify-between">
                        <span className="text-slate-500">Unrealized:</span>
                        <span className={`font-bold ${open.unrealized >= 0 ? "text-profit" : "text-loss"}`}>
                          {usd(open.unrealized)}
                        </span>
                      </div>
                    )}
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
        ) : (
          <EmptyState
            title="No Strategy Activity"
            hint="Closed cycles will attribute realized PnL here per strategy."
          />
        )}
      </div>
    </div>
  );
};

export const PortfolioMonitor: React.FC<PortfolioMonitorProps> = React.memo(PortfolioMonitorInner);
