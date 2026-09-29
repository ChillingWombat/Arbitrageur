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
  Boxes,
  Radar,
  PieChart,
} from "lucide-react";
import { SpreadGapIcon } from "./icons";
import { EmptyState } from "./EmptyState";
import { ConfirmActionModal } from "./ConfirmActionModal";
import { VenueLogo } from "./VenueLogo";
import { ExchangeVenueGadget } from "./ExchangeVenueGadget";
import { RebalanceEstimatorGadget } from "./RebalanceEstimatorGadget";
import { ExternalPositionsGadget } from "./ExternalPositionsGadget";
import { TradedOpportunitiesGadget } from "./TradedOpportunitiesGadget";
import { NetPnlChartGadget } from "./NetPnlChartGadget";
import { formatStrategyName, ledgerStrategyId } from "../types/tickers";
import { api } from "../services/api";
import { computeExchangeStats } from "../services/exchangeStats";
import type {
  ExecutionLogEntry,
  OpenPositionDto,
  RiskOverviewDto,
  SystemStatusDto,
  StrategyMetrics,
  TradedOpportunityDto,
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
  /** Command failures surface as toasts — never swallowed to console. */
  onCommandError?: (message: string) => void;
}

interface AttentionItem {
  severity: "critical" | "warn";
  title: string;
  detail: string;
}

const usd = (v: number, digits = 2) => {
  // Unknown stays unknown — a NaN mark must never read as "$0.00".
  if (!Number.isFinite(v)) return "—";
  if (Math.abs(v) < 0.005) return `$0.00`;
  return `${v >= 0 ? "+" : "-"}$${Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;
};
const usdPlain = (v: number, digits = 2) => {
  if (!Number.isFinite(v)) return "$—";
  return `$${v.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;
};
const formatVenueName = (raw: string) => {
  const v = (raw ?? "").trim();
  if (!v) return "Unknown";
  const lower = v.toLowerCase();
  const known: Record<string, string> = {
    hyperliquid: "Hyperliquid",
    lighter: "Lighter",
    robinhood: "Robinhood",
    vanta: "Vanta",
    aster: "Aster",
    popdex: "PopDEX",
    binance: "Binance",
    okx: "OKX",
    coinbase: "Coinbase",
    alpaca: "Alpaca",
    kraken: "Kraken",
    decibel: "Decibel",
    arcus: "Arcus",
  };
  if (known[lower]) return known[lower];
  return v.charAt(0).toUpperCase() + v.slice(1);
};
const formatShare = (sharePct: number, equityUsd: number) => {
  if (!(equityUsd > 0) || !(sharePct > 0)) return "0%";
  if (sharePct < 0.5) return "<1%";
  if (sharePct < 10) return `${sharePct.toFixed(1)}%`;
  return `${sharePct.toFixed(0)}%`;
};
const pnlTone = (v: number) =>
  !Number.isFinite(v) || Math.abs(v) < 0.005 ? "text-slate-500" : v > 0 ? "text-profit" : "text-loss";

export interface StrategyButtonItem {
  id: string;
  label: string;
}

/**
 * Group any strategy ID starting with options_ into "options" (display label "Options").
 */
export const groupStrategyId = (strategyId: string): string =>
  strategyId.startsWith("options_") ? "options" : strategyId;

export const STRATEGY_BUTTONS: StrategyButtonItem[] = [
  { id: "all", label: "All" },
  { id: "spatial_spread", label: "Simple Spread" },
  { id: "radar", label: "Spread Radar" },
  { id: "funding_rate", label: "Funding Rate" },
  { id: "spot", label: "Spatial" },
  { id: "options", label: "Options" },
  { id: "prediction_parity", label: "Prediction" },
  { id: "stat_arb", label: "Statistical" },
];

export const matchStrategy = (rawId: string, filter: string): boolean => {
  if (filter === "all") return true;
  const canonicalFilter = ledgerStrategyId(filter);
  const canonicalId = ledgerStrategyId(rawId);
  if (filter === "options" || canonicalFilter === "options") {
    return (
      rawId === "options" ||
      rawId.startsWith("options_") ||
      canonicalId === "options" ||
      canonicalId.startsWith("options_")
    );
  }
  return (
    canonicalId === canonicalFilter ||
    rawId.toLowerCase() === filter.toLowerCase() ||
    (canonicalFilter === "spot" && (canonicalId === "spot_trade" || canonicalId === "spot_arb"))
  );
};

const PALETTE = [
  "#2563eb", // blue-600
  "#0d9488", // teal-600
  "#8b5cf6", // violet-500
  "#f59e0b", // amber-500
  "#10b981", // emerald-500
  "#ec4899", // pink-500
  "#6366f1", // indigo-500
  "#f97316", // orange-500
  "#06b6d4", // cyan-500
  "#84cc16", // lime-500
  "#64748b", // slate-500
];

const VENUE_COLORS: Record<string, string> = {
  deribit: "#0284c7",
  alpaca: "#f59e0b",
  lighter: "#6366f1",
  hyperliquid: "#10b981",
  decibel: "#eab308",
  robinhood: "#22c55e",
  vanta: "#8b5cf6",
  binance: "#f59e0b",
  okx: "#334155",
  coinbase: "#2563eb",
  kraken: "#7c3aed",
  aster: "#ec4899",
  popdex: "#14b8a6",
  arcus: "#3b82f6",
};

const STRATEGY_COLORS: Record<string, string> = {
  spatial_spread: "#2563eb",
  futures: "#2563eb",
  radar: "#0284c7",
  funding_rate: "#059669",
  funding: "#059669",
  spot: "#d97706",
  options: "#7c3aed",
  prediction_parity: "#9333ea",
  prediction: "#9333ea",
  stat_arb: "#4f46e5",
  unattributed: "#64748b",
};

interface DonutSlice {
  id: string;
  label: string;
  value: number;
  share: number;
  color: string;
  sublabel?: string;
}

function polarToCartesian(cx: number, cy: number, r: number, angleDeg: number) {
  const rad = ((angleDeg - 90) * Math.PI) / 180.0;
  return {
    x: Number((cx + r * Math.cos(rad)).toFixed(2)),
    y: Number((cy + r * Math.sin(rad)).toFixed(2)),
  };
}

function describeArc(
  cx: number,
  cy: number,
  r: number,
  innerR: number,
  startAngle: number,
  endAngle: number,
): string {
  if (endAngle - startAngle >= 359.9) {
    return [
      `M ${cx} ${cy - r}`,
      `A ${r} ${r} 0 1 1 ${cx} ${cy + r}`,
      `A ${r} ${r} 0 1 1 ${cx} ${cy - r}`,
      `M ${cx} ${cy - innerR}`,
      `A ${innerR} ${innerR} 0 1 0 ${cx} ${cy + innerR}`,
      `A ${innerR} ${innerR} 0 1 0 ${cx} ${cy - innerR}`,
      "Z",
    ].join(" ");
  }

  const p1 = polarToCartesian(cx, cy, r, startAngle);
  const p2 = polarToCartesian(cx, cy, r, endAngle);
  const p3 = polarToCartesian(cx, cy, innerR, endAngle);
  const p4 = polarToCartesian(cx, cy, innerR, startAngle);
  const largeArcFlag = endAngle - startAngle <= 180 ? "0" : "1";

  return [
    `M ${p1.x} ${p1.y}`,
    `A ${r} ${r} 0 ${largeArcFlag} 1 ${p2.x} ${p2.y}`,
    `L ${p3.x} ${p3.y}`,
    `A ${innerR} ${innerR} 0 ${largeArcFlag} 0 ${p4.x} ${p4.y}`,
    "Z",
  ].join(" ");
}

const DonutChart: React.FC<{
  slices: DonutSlice[];
  totalValue: number;
  totalLabel: string;
  totalSublabel?: string;
  hoveredId: string | null;
  onHover: (id: string | null) => void;
  size?: number;
}> = ({
  slices,
  totalValue,
  totalLabel,
  totalSublabel,
  hoveredId,
  onHover,
  size = 180,
}) => {
  const cx = size / 2;
  const cy = size / 2;
  const r = size / 2 - 10;
  const innerR = r - 26;

  const activeSlices = slices.filter((s) => s.value > 0);
  const sumVal = activeSlices.reduce((acc, s) => acc + s.value, 0);

  const arcs = useMemo(() => {
    if (activeSlices.length === 0) return [];
    if (activeSlices.length === 1) {
      return [{ ...activeSlices[0], startAngle: 0, endAngle: 360 }];
    }
    const minDeg = 3.5;
    const smallCount = activeSlices.filter((s) => (s.value / sumVal) * 360 < minDeg).length;
    const largeVal = activeSlices
      .filter((s) => (s.value / sumVal) * 360 >= minDeg)
      .reduce((acc, s) => acc + s.value, 0);
    const remainingDeg = Math.max(0, 360 - smallCount * minDeg);

    let curAngle = 0;
    return activeSlices.map((s) => {
      let angleSpan = (s.value / sumVal) * 360;
      if (angleSpan < minDeg && smallCount > 0 && largeVal > 0) {
        angleSpan = minDeg;
      } else if (largeVal > 0 && smallCount > 0) {
        angleSpan = (s.value / largeVal) * remainingDeg;
      }
      const start = curAngle;
      const end = curAngle + angleSpan;
      curAngle = end;
      return { ...s, startAngle: start, endAngle: end };
    });
  }, [activeSlices, sumVal]);

  const hoveredSlice = hoveredId ? slices.find((s) => s.id === hoveredId) : null;

  return (
    <div className="relative flex items-center justify-center select-none" style={{ width: size, height: size }}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="overflow-visible">
        {arcs.length === 0 ? (
          <circle
            cx={cx}
            cy={cy}
            r={(r + innerR) / 2}
            stroke="#e2e8f0"
            strokeWidth={r - innerR}
            fill="none"
          />
        ) : (
          arcs.map((a) => {
            const isHovered = hoveredId === a.id;
            const currentR = isHovered ? r + 3 : r;
            const currentInnerR = isHovered ? innerR - 2 : innerR;
            const d = describeArc(cx, cy, currentR, currentInnerR, a.startAngle, a.endAngle);
            return (
              <path
                key={a.id}
                d={d}
                fill={a.color}
                stroke="#ffffff"
                strokeWidth={arcs.length > 1 ? 1.5 : 0}
                className="transition-all duration-200 cursor-pointer"
                style={{
                  opacity: hoveredId && !isHovered ? 0.45 : 1,
                  filter: isHovered ? "drop-shadow(0 2px 4px rgba(0,0,0,0.18))" : "none",
                }}
                onMouseEnter={() => onHover(a.id)}
                onMouseLeave={() => onHover(null)}
              />
            );
          })
        )}
      </svg>

      {/* Center Label */}
      <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none text-center px-3">
        {hoveredSlice ? (
          <>
            <span className="text-[11px] font-mono font-bold text-slate-700 truncate max-w-[95px]">
              {hoveredSlice.label}
            </span>
            <span className="text-xs font-bold font-mono text-slate-900 tabular-nums">
              {usdPlain(hoveredSlice.value)}
            </span>
            <span className="text-[10px] font-mono font-bold text-accent-strong">
              {hoveredSlice.sublabel ?? `${hoveredSlice.share.toFixed(1)}%`}
            </span>
          </>
        ) : (
          <>
            <span className="text-xs font-bold font-mono text-slate-900 tabular-nums tracking-tight">
              {usdPlain(totalValue)}
            </span>
            <span className="text-[10px] font-mono text-muted uppercase tracking-wider mt-0.5">
              {totalLabel}
            </span>
            {totalSublabel && (
              <span className="text-[9px] font-mono text-slate-400 mt-0.5">
                {totalSublabel}
              </span>
            )}
          </>
        )}
      </div>
    </div>
  );
};

const PortfolioMonitorInner: React.FC<PortfolioMonitorProps> = ({
  riskData,
  status,
  openPositions = [],
  onResetCircuitBreaker,
  onTriggerKillswitch,
  onCommandError,
}) => {
  const [showHaltConfirm, setShowHaltConfirm] = useState(false);
  const [metrics, setMetrics] = useState<StrategyMetrics[]>([]);
  const [tradedOpps, setTradedOpps] = useState<TradedOpportunityDto[]>([]);
  const [strategyFilter, setStrategyFilter] = useState<string>("all");
  const [allocationTab, setAllocationTab] = useState<"venue" | "strategy">("venue");
  const [hoveredSliceId, setHoveredSliceId] = useState<string | null>(null);
  // Per-venue inputs for the exchange gadget. Null until the first
  // successful read (never an empty fake).
  const [execLogs, setExecLogs] = useState<ExecutionLogEntry[] | null>(null);
  const [venueBalances, setVenueBalances] = useState<VenueBalanceInfo[] | null>(null);
  // Per-venue 24h utilisation histories keyed by lowercase venue id. Absent
  // until the first successful read — the gadget keeps the "—" fallback.
  const [utilHistories, setUtilHistories] = useState<Record<string, UtilisationHistory | null>>({});
  // Last manual reconciliation-pass summary; null until the operator runs one.
  const [reconcileNote, setReconcileNote] = useState<string | null>(null);
  const [reconciling, setReconciling] = useState(false);

  const loadStrategyMetrics = async () => {
    try {
      const data = await api.getStrategyPerformance();
      // A malformed response (e.g. an object instead of a row array) must not
      // crash the monitor — degrade to the empty list instead.
      setMetrics(Array.isArray(data) ? data : []);
    } catch (err) {
      console.error("Failed to load strategy performance:", err);
    }
  };

  const loadTradedOpps = async () => {
    try {
      const data = await api.listTradedOpportunities();
      setTradedOpps(Array.isArray(data) ? data : []);
    } catch (err) {
      console.error("Failed to load traded opportunities:", err);
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
    loadTradedOpps();
    const interval = setInterval(() => {
      loadStrategyMetrics();
      loadExchangeInputs();
      loadTradedOpps();
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

  // Backend-unknown is NOT healthy: when neither the status snapshot nor a
  // risk overview has arrived we cannot claim trading state — render
  // "Unknown" instead of defaulting to a healthy/Halted binary.
  const backendKnown = status != null || riskData != null;
  const isTripped = status?.circuit_breaker_tripped ?? riskData?.is_halted ?? false;
  const isPaused = status?.is_paused ?? false;
  const margin = riskData?.margin_health;

  // --- Derived portfolio truth -------------------------------------------------
  const equity = riskData?.total_portfolio_value_usd ?? 0;
  // Open positions scoped to the selected strategy
  const visibleOpenPositions = useMemo(
    () =>
      strategyFilter === "all"
        ? openPositions
        : openPositions.filter((p) => matchStrategy(p.strategyId ?? "", strategyFilter)),
    [openPositions, strategyFilter],
  );
  const markedPositions = useMemo(
    () => visibleOpenPositions.filter((p) => p.unrealizedPnlUsd != null),
    [visibleOpenPositions],
  );
  const unrealizedUsd = useMemo(
    () => markedPositions.reduce((acc, p) => acc + (p.unrealizedPnlUsd ?? 0), 0),
    [markedPositions],
  );
  const deployedUsd = useMemo(
    () =>
      visibleOpenPositions.reduce(
        (acc, p) => acc + (p.notionalUsd != null && p.notionalUsd > 0 ? p.notionalUsd : 0),
        0,
      ),
    [visibleOpenPositions],
  );
  const idleUsd = Math.max(0, equity - deployedUsd);

  // Metrics scoped to the selected strategy ("all" = aggregate view)
  const visibleMetrics = useMemo(
    () =>
      strategyFilter === "all"
        ? metrics
        : metrics.filter((m) =>
            strategyFilter === "options"
              ? m.strategy_id === "options" || m.strategy_id.startsWith("options_")
              : matchStrategy(m.strategy_id, strategyFilter),
          ),
    [metrics, strategyFilter],
  );

  // Aggregate strategy metrics (respect the monitor selector)
  const totalCompleted = visibleMetrics.reduce((acc, m) => acc + m.total_cycles_completed, 0);
  const totalWinning = visibleMetrics.reduce((acc, m) => acc + m.winning_cycles, 0);
  const totalPnlUsd = visibleMetrics.reduce((acc, m) => acc + m.total_pnl_usd, 0);
  const overallWinRate = totalCompleted > 0 ? (totalWinning / totalCompleted) * 100 : 0;

  // One row per ledger strategy namespace: union of cycle-metric rows,
  // dispatched signals, and live open positions. Signal ids normalize into
  // their lot namespace (futures→spatial_spread, funding→funding_rate) so a
  // strategy's metrics, signals, and positions land on one row — strategies
  // like the options substrategies appear even without cycle metrics.
  const strategyRows = useMemo(() => {
    interface Row {
      ledgerId: string;
      signals: number;
      openCount: number;
      realizedUsd: number;
      metricsPnlUsd: number | null;
      unrealizedUsd: number;
      cycles: number | null;
      wins: number | null;
      sharpe: number | null;
      apr: number | null;
      lastActivityMs: number | null;
    }
    const rows = new Map<string, Row>();
    const slot = (rawId: string): Row => {
      const id = ledgerStrategyId(rawId);
      let r = rows.get(id);
      if (!r) {
        r = {
          ledgerId: id, signals: 0, openCount: 0, realizedUsd: 0,
          metricsPnlUsd: null, unrealizedUsd: 0, cycles: null, wins: null,
          sharpe: null, apr: null, lastActivityMs: null,
        };
        rows.set(id, r);
      }
      return r;
    };
    for (const m of metrics) {
      const r = slot(m.strategy_id);
      r.metricsPnlUsd = m.total_pnl_usd;
      r.cycles = m.total_cycles_completed;
      r.wins = m.winning_cycles;
      r.sharpe = m.sharpe_ratio;
      r.apr = m.annualized_apr_pct;
    }
    for (const o of tradedOpps) {
      const r = slot(o.strategyId);
      r.signals += 1;
      if (o.realizedPnlUsd != null) r.realizedUsd += o.realizedPnlUsd;
      const ts = Date.parse(o.closedAt ?? o.createdAt);
      if (Number.isFinite(ts)) r.lastActivityMs = Math.max(r.lastActivityMs ?? ts, ts);
    }
    for (const p of openPositions) {
      const r = slot(p.strategyId ?? "unknown");
      r.openCount += 1;
      if (p.unrealizedPnlUsd != null) r.unrealizedUsd += p.unrealizedPnlUsd;
    }
    return [...rows.values()].sort(
      (a, b) =>
        (b.openCount > 0 ? 1 : 0) - (a.openCount > 0 ? 1 : 0) ||
        Math.abs(b.realizedUsd) + Math.abs(b.unrealizedUsd) - (Math.abs(a.realizedUsd) + Math.abs(a.unrealizedUsd)) ||
        b.signals - a.signals,
    );
  }, [metrics, tradedOpps, openPositions]);

  const visibleStrategyRows = useMemo(
    () =>
      strategyFilter === "all"
        ? strategyRows
        : strategyRows.filter((r) =>
            strategyFilter === "options"
              ? r.ledgerId === "options" || r.ledgerId.startsWith("options_")
              : matchStrategy(r.ledgerId, strategyFilter),
          ),
    [strategyRows, strategyFilter],
  );

  // --- Attention items: what needs the user right now ----------------------------
  const attention: AttentionItem[] = useMemo(() => {
    const items: AttentionItem[] = [];
    if (!backendKnown)
      items.push({
        severity: "warn",
        title: "Backend state unavailable",
        detail: "No status or risk snapshot has loaded — trading state is unknown, not verified healthy.",
      });
    if (isTripped)
      items.push({
        severity: "critical",
        title: "Halted — circuit breaker tripped",
        detail: status?.circuit_breaker_status || riskData?.circuit_breaker_state || "Manual halt engaged.",
      });
    // Flat book has no liquidation risk — suppress buffer warnings when there
    // is nothing to liquidate (backend reports 100% when flat, but guard here
    // too for stale/mixed snapshots).
    const flatBook =
      (riskData?.margin_health?.status ?? margin?.status ?? "") === "FLAT" ||
      openPositions.length === 0;
    if (margin != null) {
      if (margin.margin_ratio_pct >= 90)
        items.push({ severity: "critical", title: `Margin utilization ${margin.margin_ratio_pct.toFixed(0)}%`, detail: "Above 90% — reduce exposure or add margin before the next volatility spike." });
      else if (margin.margin_ratio_pct >= 70)
        items.push({ severity: "warn", title: `Margin utilization ${margin.margin_ratio_pct.toFixed(0)}%`, detail: "Climbing — watch new entries sizing." });
      if (!flatBook) {
        if (margin.liquidation_distance_pct <= 10)
          items.push({ severity: "critical", title: `Liquidation buffer ${margin.liquidation_distance_pct.toFixed(1)}%`, detail: "Dangerously thin — trim or top up immediately." });
        else if (margin.liquidation_distance_pct <= 25)
          items.push({ severity: "warn", title: `Liquidation buffer ${margin.liquidation_distance_pct.toFixed(1)}%`, detail: "Narrowing — avoid adding leverage." });
      }
    }
    const skewed = (riskData?.net_deltas ?? []).filter((d) => Math.abs(d.net_exposure_usd) > d.delta_limit_usd);
    for (const d of skewed.slice(0, 3))
      items.push({ severity: "warn", title: `${d.asset} exposure skew ${usd(d.net_exposure_usd)}`, detail: `Beyond the $${d.delta_limit_usd.toLocaleString("en-US")} delta-neutral tolerance.` });
    if (skewed.length > 3)
      items.push({ severity: "warn", title: `${skewed.length - 3} more skewed assets`, detail: "See the DeltaGuard table below." });
    const fails = riskData?.consecutive_failures ?? 0;
    const allowed = riskData?.max_failures_allowed ?? 3;
    if (allowed > 0 && fails >= allowed - 1 && fails > 0)
      items.push({ severity: "warn", title: `Error budget nearly exhausted (${fails}/${allowed})`, detail: "One more consecutive failure trips the breaker." });
    for (const b of venueBalances ?? []) {
      // EMPTY (unfunded, flat) and STALE (transient read failure serving
      // last-known figures) are informational — never Needs Attention items.
      // STALE escalates to ERROR backend-side past the grace period, and
      // ERROR does surface on its venue card.
      if (b.status === "EMPTY" || b.status === "STALE") continue;
      if (b.status === "DEPLETED")
        items.push({ severity: "critical", title: `${formatVenueName(b.venue)} balance depleted`, detail: b.recommended_action ?? "Top up or disable this venue." });
      else if (b.status === "LOW_MARGIN")
        items.push({ severity: "warn", title: `${formatVenueName(b.venue)} low margin`, detail: b.recommended_action ?? "Reduce sizing on this venue." });
    }
    return items;
  }, [isTripped, status, riskData, margin, venueBalances, openPositions]);

  // Canonical trading-state wording: Halted = circuit breaker tripped
  // (is_halted and circuit_breaker_tripped are the same boolean). Paused =
  // engine paused, positions preserved. Unknown = no backend snapshot —
  // claiming "Healthy" with no data is how halted engines get missed.
  const verdict = !backendKnown
    ? { label: "Unknown", cls: "bg-surface-hover text-slate-500 border-border" }
    : isTripped
    ? { label: "Halted", cls: "bg-loss-dim text-loss border-loss-border" }
    : isPaused
      ? { label: "Paused", cls: "bg-warn-dim text-warn-strong border-warn-border" }
      : attention.length > 0
        ? { label: "Attention", cls: "bg-warn-dim text-warn-strong border-warn-border" }
        : { label: "Healthy", cls: "bg-profit-dim text-profit border-profit-border" };

  const venueTotal = (venueBalances ?? []).reduce((acc, b) => acc + (b.equity_usd > 0 ? b.equity_usd : 0), 0);
  const sortedVenues = useMemo(
    () => [...(venueBalances ?? [])].sort((a, b) => (b.equity_usd ?? 0) - (a.equity_usd ?? 0)),
    [venueBalances],
  );

  // Allocation slices: by venue (from balances) and by strategy (from open positions)
  const venueSlices: DonutSlice[] = useMemo(() => {
    return (sortedVenues ?? [])
      .filter((b) => Number.isFinite(b.equity_usd) && b.equity_usd > 0)
      .map((b, i) => {
        const equityUsd = Math.max(0, b.equity_usd);
        const share = venueTotal > 0 ? (equityUsd / venueTotal) * 100 : 0;
        const vid = (b.venue ?? "").toLowerCase();
        return {
          id: `${vid}-${b.account_id ?? "default"}`,
          label: formatVenueName(b.venue),
          value: equityUsd,
          share,
          color: VENUE_COLORS[vid] ?? PALETTE[i % PALETTE.length],
          sublabel: formatShare(share, equityUsd),
        };
      });
  }, [sortedVenues, venueTotal]);

  const strategyAllocations = useMemo(() => {
    const map = new Map<
      string,
      { notionalUsd: number; count: number; unrealizedUsd: number }
    >();
    for (const p of openPositions) {
      const rawId = p.strategyId ?? "unattributed";
      const id = rawId.startsWith("options_") ? "options" : ledgerStrategyId(rawId);
      const cur = map.get(id) ?? { notionalUsd: 0, count: 0, unrealizedUsd: 0 };
      const notional = p.notionalUsd != null && p.notionalUsd > 0 ? p.notionalUsd : 0;
      cur.notionalUsd += notional;
      cur.count += 1;
      if (p.unrealizedPnlUsd != null) {
        cur.unrealizedUsd += p.unrealizedPnlUsd;
      }
      map.set(id, cur);
    }
    const total = [...map.values()].reduce((acc, s) => acc + s.notionalUsd, 0);
    return [...map.entries()]
      .map(([id, s], i) => ({
        id,
        label: id === "options" ? "Options" : formatStrategyName(id),
        notionalUsd: s.notionalUsd,
        count: s.count,
        unrealizedUsd: s.unrealizedUsd,
        share: total > 0 ? (s.notionalUsd / total) * 100 : openPositions.length > 0 ? (s.count / openPositions.length) * 100 : 0,
        color: STRATEGY_COLORS[id] ?? PALETTE[i % PALETTE.length],
      }))
      .sort((a, b) => b.notionalUsd - a.notionalUsd);
  }, [openPositions]);

  const totalStrategyDeployed = useMemo(
    () => strategyAllocations.reduce((acc, s) => acc + s.notionalUsd, 0),
    [strategyAllocations],
  );

  const strategySlices: DonutSlice[] = useMemo(() => {
    return strategyAllocations.map((s) => ({
      id: s.id,
      label: s.label,
      value: s.notionalUsd > 0 ? s.notionalUsd : s.count,
      share: s.share,
      color: s.color,
      sublabel: `${s.share.toFixed(1)}%`,
    }));
  }, [strategyAllocations]);

  const getStrategyIcon = (id: string) => {
    if (id === "options" || id.startsWith("options_"))
      return <Boxes className="w-4 h-4 text-violet-600" />;
    switch (id) {
      case "spatial_spread":
      case "futures":
        return <SpreadGapIcon className="w-4 h-4 text-blue-600" />;
      case "funding":
      case "funding_rate":
        return <Percent className="w-4 h-4 text-emerald-700" />;
      case "spot_trade":
      case "spot_arb":
      case "spot":
        return <Coins className="w-4 h-4 text-amber-700" />;
      case "triangular_arb":
      case "triangle":
        return <Triangle className="w-4 h-4 text-teal-700" />;
      case "prediction":
      case "prediction_parity":
        return <Target className="w-4 h-4 text-purple-600" />;
      case "stat_arb":
        return <Sigma className="w-4 h-4 text-indigo-600" />;
      case "radar":
        return <Radar className="w-4 h-4 text-sky-600" />;
      default:
        return <Activity className="w-4 h-4 text-slate-600" />;
    }
  };

  // Strategy display names come from the canonical STRATEGY_DISPLAY_NAMES map
  // (types/tickers) — keyed by the persisted ids: futures/funding/spot/etc.

  return (
    <div className="w-full p-4 pr-5 space-y-4 overflow-y-auto h-full select-none pb-24">
      {/* 1. Strategy Switcher & Key Metrics Gadget Zone */}
      <div className="space-y-3">
        {/* Strategy Switcher Button Row */}
        <div className="bg-surface p-2.5 rounded-xl border border-surface-border flex items-center justify-between gap-2 flex-wrap shadow-card">
          <div className="flex items-center gap-1.5 flex-wrap">
            <span className="text-xs font-mono font-bold text-slate-500 uppercase tracking-wider pl-1 pr-1.5">
              Strategy:
            </span>
            {STRATEGY_BUTTONS.map((s) => {
              const active = strategyFilter === s.id;
              return (
                <button
                  key={s.id}
                  onClick={() => setStrategyFilter(s.id)}
                  className={`px-3 py-1.5 rounded-lg text-xs font-mono font-semibold transition cursor-pointer flex items-center gap-1.5 ${
                    active
                      ? "bg-accent text-white shadow-xs"
                      : "bg-surface-raised hover:bg-slate-200/70 text-slate-600 hover:text-slate-900 border border-surface-border"
                  }`}
                  title={`Filter portfolio metrics to ${s.label}`}
                >
                  {s.id !== "all" && getStrategyIcon(s.id)}
                  <span>{s.label}</span>
                </button>
              );
            })}
          </div>

          {isTripped && (
            <button
              onClick={onResetCircuitBreaker}
              className="px-3 py-1.5 bg-warn-dim hover:bg-amber-100 text-warn-strong border border-warn-border rounded-lg text-xs font-mono font-bold flex items-center gap-1.5 transition shadow-xs cursor-pointer shrink-0"
              title="Reset Circuit Breaker — Resume Trading"
            >
              <span>Reset Circuit Breaker</span>
            </button>
          )}
        </div>

        {/* Key Metrics Little Gadgets */}
        <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3">
          {/* 1. Net Equity */}
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-muted font-mono">Net Equity</span>
              <Wallet className="w-4 h-4 text-accent" />
            </div>
            <div className="text-lg font-bold text-slate-900 font-mono tabular-nums tracking-tight">
              {usdPlain(equity)}
            </div>
            <div className="text-xs text-slate-500 font-mono truncate">
              {venueBalances ? `${venueBalances.length} venue${venueBalances.length === 1 ? "" : "s"} connected` : "Venue balances"}
            </div>
          </div>

          {/* 2. Realized PnL */}
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-muted font-mono">Realized P&L</span>
              <TrendingUp className="w-4 h-4 text-emerald-600" />
            </div>
            <div className={`text-lg font-bold font-mono tabular-nums ${pnlTone(totalPnlUsd)}`}>
              {usd(totalPnlUsd)}
            </div>
            <div className="text-xs text-slate-500 font-mono truncate">
              {totalCompleted > 0 ? `${totalCompleted} cycle${totalCompleted === 1 ? "" : "s"} (${overallWinRate.toFixed(0)}% win)` : "no closed cycles"}
            </div>
          </div>

          {/* 3. Unrealized PnL */}
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-muted font-mono">Unrealized P&L</span>
              <Activity className="w-4 h-4 text-sky-600" />
            </div>
            <div className={`text-lg font-bold font-mono tabular-nums ${pnlTone(unrealizedUsd)}`}>
              {markedPositions.length > 0 ? usd(unrealizedUsd) : "$0.00"}
            </div>
            <div className="text-xs text-slate-500 font-mono truncate">
              {markedPositions.length > 0 ? `${markedPositions.length} marked (${visibleOpenPositions.length} open)` : "flat (no marks)"}
            </div>
          </div>

          {/* 4. Deployed / Margin Utilization */}
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-muted font-mono">Margin / Deployed</span>
              <Percent className="w-4 h-4 text-indigo-600" />
            </div>
            <div className="text-lg font-bold text-slate-900 font-mono tabular-nums">
              {margin ? `${margin.margin_ratio_pct.toFixed(0)}%` : "0%"}
            </div>
            <div className="text-xs text-slate-500 font-mono truncate">
              {usdPlain(deployedUsd)} deployed
            </div>
          </div>

          {/* 5. Trading Status */}
          <div className="bg-surface p-3.5 rounded-xl border border-surface-border shadow-card space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-xs font-medium text-muted font-mono">Trading Status</span>
              <ShieldCheck className="w-4 h-4 text-accent-strong" />
            </div>
            <div className="mt-0.5">
              <span
                className={`inline-block px-2.5 py-0.5 rounded-md text-xs font-mono font-bold border tracking-wider ${verdict.cls}`}
              >
                {verdict.label}
              </span>
            </div>
            <div className="text-xs text-slate-500 font-mono truncate">
              {isTripped
                ? "Circuit breaker tripped"
                : isPaused
                ? "Engine paused"
                : attention.length > 0
                ? `${attention.length} need${attention.length === 1 ? "s" : ""} review`
                : "Active & healthy"}
            </div>
          </div>
        </div>
      </div>

      {/* 2. Strategy Performance Attribution Breakdown (Moved upwards to be 2nd gadget from top) */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
        <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
          <div>
            <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
              <TrendingUp className="w-5 h-5 text-accent-strong" />
              <span>Multi-Strategy PnL & Cycle Attribution</span>
            </h2>
          </div>
          <span className="text-xs font-mono font-bold text-slate-600">
            {visibleStrategyRows.length} {visibleStrategyRows.length === 1 ? "Strategy" : "Strategies"} Active
          </span>
        </div>

        {visibleStrategyRows.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs font-mono tabular-nums">
              <thead className="bg-surface-raised/60">
                <tr className="border-b border-surface-border text-slate-500 text-xs font-semibold tracking-wider">
                  <th className="py-2.5 px-3">Strategy</th>
                  <th className="py-2.5 px-3 text-right" title="Dispatched signals in the traded-opportunity ledger">Signals</th>
                  <th className="py-2.5 px-3 text-right" title="Live open legs attributed to this strategy">Open</th>
                  <th className="py-2.5 px-3 text-right" title="Fee-inclusive net P&L across closed ledger rows">Realized P&L</th>
                  <th className="py-2.5 px-3 text-right" title="Unrealized P&L on live open legs">Unrealized</th>
                  <th className="py-2.5 px-3 text-right" title="Winning / completed cycles (cycle-ledger strategies only)">Win Rate</th>
                  <th className="py-2.5 px-3 text-right">Sharpe</th>
                  <th className="py-2.5 px-3 text-right">APR</th>
                  <th className="py-2.5 px-3 text-right" title="Most recent signal dispatch or close">Last Activity</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-border">
                {visibleStrategyRows.map((r) => {
                  const realized = r.signals > 0 || r.metricsPnlUsd == null ? r.realizedUsd : r.metricsPnlUsd;
                  const winRate =
                    r.cycles != null && r.cycles > 0 && r.wins != null
                      ? (r.wins / r.cycles) * 100
                      : null;
                  return (
                    <tr key={r.ledgerId} className="hover:bg-surface-raised/40 transition">
                      <td className="py-2.5 px-3">
                        <div className="flex items-center space-x-2.5">
                          <span className="p-1.5 bg-surface rounded-md border border-surface-border shadow-xs">
                            {getStrategyIcon(r.ledgerId)}
                          </span>
                          <div className="min-w-0">
                            <div className="font-bold text-[13px] text-slate-900 truncate">{formatStrategyName(r.ledgerId)}</div>
                            <div className="text-[11px] text-muted">{r.ledgerId}</div>
                          </div>
                        </div>
                      </td>
                      <td className="py-2.5 px-3 text-right text-slate-700">{r.signals}</td>
                      <td className="py-2.5 px-3 text-right">
                        {r.openCount > 0 ? (
                          <span className="px-1.5 py-0.5 rounded-md text-[11px] font-bold bg-accent-dim text-accent-strong border border-accent-line/60">
                            {r.openCount} open
                          </span>
                        ) : (
                          <span className="text-slate-400">0</span>
                        )}
                      </td>
                      <td className="py-2.5 px-3 text-right">
                        <span className={`font-bold ${pnlTone(realized)}`}>{usd(realized)}</span>
                      </td>
                      <td className="py-2.5 px-3 text-right">
                        {r.openCount > 0 ? (
                          <span className={`font-bold ${pnlTone(r.unrealizedUsd)}`}>{usd(r.unrealizedUsd)}</span>
                        ) : (
                          <span className="text-muted">—</span>
                        )}
                      </td>
                      <td className="py-2.5 px-3 text-right text-slate-800">
                        {winRate != null ? `${winRate.toFixed(1)}% (${r.wins}/${r.cycles})` : "—"}
                      </td>
                      <td className="py-2.5 px-3 text-right text-accent-strong font-bold">
                        {r.sharpe != null ? r.sharpe.toFixed(2) : "—"}
                      </td>
                      <td className="py-2.5 px-3 text-right text-slate-700">
                        {r.apr != null ? `${r.apr.toFixed(1)}%` : "—"}
                      </td>
                      <td className="py-2.5 px-3 text-right text-muted">
                        {r.lastActivityMs != null
                          ? new Date(r.lastActivityMs).toLocaleString(undefined, {
                              month: "short",
                              day: "numeric",
                              hour: "2-digit",
                              minute: "2-digit",
                            })
                          : "—"}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : (
          <EmptyState
            title="No Strategy Activity"
            hint="Signals, open positions, or closed cycles will attribute here per strategy."
          />
        )}
      </div>

      {/* 2. Attention: what needs the user, worst first — above capital so halts can't be missed */}
      <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
        <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
          <Eye className="w-5 h-5 text-accent-strong" />
          <span>Needs Attention</span>
          {attention.length > 0 && (
            <span className="px-1.5 py-0.5 rounded-md text-[11px] font-mono font-bold min-w-[16px] text-center leading-tight bg-accent-dim text-accent-strong border border-accent-line/60">
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
                <AlertTriangle className={`w-4 h-4 shrink-0 mt-0.5 ${a.severity === "critical" ? "text-loss" : "text-warn-strong"}`} />
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


      {/* 3. Capital: where the money sits + deployed vs idle */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-4">
        <div className="xl:col-span-2 bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
          <div className="flex items-center justify-between border-b border-surface-border pb-2.5 flex-wrap gap-2">
            <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
              <PieChart className="w-5 h-5 text-accent-strong" />
              <span>Capital Allocation</span>
            </h2>
            <div className="flex items-center bg-surface-raised p-0.5 rounded-lg border border-surface-border text-xs font-mono">
              <button
                type="button"
                onClick={() => {
                  setAllocationTab("venue");
                  setHoveredSliceId(null);
                }}
                className={`px-3 py-1 rounded-md font-semibold transition cursor-pointer ${
                  allocationTab === "venue"
                    ? "bg-surface text-slate-900 shadow-2xs font-bold"
                    : "text-slate-500 hover:text-slate-800"
                }`}
              >
                By Venue
              </button>
              <button
                type="button"
                onClick={() => {
                  setAllocationTab("strategy");
                  setHoveredSliceId(null);
                }}
                className={`px-3 py-1 rounded-md font-semibold transition cursor-pointer ${
                  allocationTab === "strategy"
                    ? "bg-surface text-slate-900 shadow-2xs font-bold"
                    : "text-slate-500 hover:text-slate-800"
                }`}
              >
                By Strategy
              </button>
            </div>
          </div>

          {allocationTab === "venue" ? (
            sortedVenues.length > 0 ? (
              <div className="grid grid-cols-1 md:grid-cols-12 gap-4 items-center">
                {/* Donut Chart */}
                <div className="md:col-span-5 flex justify-center py-1">
                  <DonutChart
                    slices={venueSlices}
                    totalValue={venueTotal}
                    totalLabel="Venue Total"
                    totalSublabel={`${sortedVenues.length} venue${sortedVenues.length === 1 ? "" : "s"}`}
                    hoveredId={hoveredSliceId}
                    onHover={setHoveredSliceId}
                    size={175}
                  />
                </div>

                {/* Venue Breakdown List */}
                <div className="md:col-span-7 space-y-2 max-h-[260px] overflow-y-auto pr-1">
                  {sortedVenues.map((b) => {
                    const equityUsd = Number.isFinite(b.equity_usd) ? Math.max(0, b.equity_usd) : 0;
                    const freeUsd = Number.isFinite(b.available_margin_usd) ? Math.max(0, b.available_margin_usd) : 0;
                    const share = venueTotal > 0 && equityUsd > 0 ? (equityUsd / venueTotal) * 100 : 0;
                    const vid = (b.venue ?? "").toLowerCase();
                    const sliceId = `${vid}-${b.account_id ?? "default"}`;
                    const sliceColor = VENUE_COLORS[vid] ?? "#64748b";
                    const isHovered = hoveredSliceId === sliceId;
                    const showFree = freeUsd > 0 && Math.abs(freeUsd - equityUsd) > 0.005;
                    const barWidth = equityUsd > 0 ? Math.max(1.5, Math.min(100, share)) : 0;

                    return (
                      <div
                        key={sliceId}
                        onMouseEnter={() => setHoveredSliceId(sliceId)}
                        onMouseLeave={() => setHoveredSliceId(null)}
                        className={`space-y-1 p-1.5 rounded-lg transition cursor-pointer ${
                          isHovered ? "bg-surface-raised/70" : "hover:bg-surface-raised/40"
                        }`}
                      >
                        <div className="flex items-center justify-between gap-3 text-xs font-mono">
                          <span className="font-bold text-slate-900 truncate flex items-center gap-2 min-w-0">
                            <span
                              className="w-2.5 h-2.5 rounded-full shrink-0"
                              style={{ backgroundColor: sliceColor }}
                            />
                            <VenueLogo venueId={vid} name={formatVenueName(b.venue)} size={16} />
                            <span className="truncate">{formatVenueName(b.venue)}</span>
                          </span>
                          <span className="tabular-nums text-slate-700 whitespace-nowrap">
                            {usdPlain(equityUsd)}{" "}
                            <span className="text-muted">({formatShare(share, equityUsd)})</span>
                            {showFree && (
                              <span className="text-muted"> · {usdPlain(freeUsd)} free</span>
                            )}
                          </span>
                        </div>
                        <div className="h-1.5 w-full bg-slate-200 rounded-full overflow-hidden">
                          <div
                            className="h-full transition-all duration-300"
                            style={{ width: `${barWidth}%`, backgroundColor: sliceColor }}
                          />
                        </div>
                        {b.status !== "HEALTHY" && b.status !== "EMPTY" && b.status !== "STALE" && b.status !== "LOCKED" && b.recommended_action && (
                          <div className="text-[11px] font-mono text-warn-strong">{b.recommended_action}</div>
                        )}
                        {(b.status === "EMPTY" || b.status === "STALE" || b.status === "LOCKED") && b.recommended_action && (
                          <div className="text-[11px] font-mono text-muted">{b.recommended_action}</div>
                        )}
                      </div>
                    );
                  })}
                  <div className="flex items-center justify-between pt-1 text-[11px] font-mono text-slate-500 border-t border-surface-border">
                    <span>
                      Total {usdPlain(venueTotal)} across {sortedVenues.length} venue{sortedVenues.length === 1 ? "" : "s"}
                    </span>
                    {Math.abs(venueTotal - equity) > Math.max(1, equity * 0.005) && equity > 0 && (
                      <span title="Header equity comes from risk overview; venue total from venue balances.">
                        Header {usdPlain(equity)}
                      </span>
                    )}
                  </div>
                </div>
              </div>
            ) : (
              <EmptyState title="No Venue Balances" hint="Connect an exchange account to see capital allocation." />
            )
          ) : (
            strategyAllocations.length > 0 ? (
              <div className="grid grid-cols-1 md:grid-cols-12 gap-4 items-center">
                {/* Donut Chart */}
                <div className="md:col-span-5 flex justify-center py-1">
                  <DonutChart
                    slices={strategySlices}
                    totalValue={totalStrategyDeployed}
                    totalLabel="Deployed Margin"
                    totalSublabel={`${openPositions.length} open position${openPositions.length === 1 ? "" : "s"}`}
                    hoveredId={hoveredSliceId}
                    onHover={setHoveredSliceId}
                    size={175}
                  />
                </div>

                {/* Strategy Breakdown List */}
                <div className="md:col-span-7 space-y-2 max-h-[260px] overflow-y-auto pr-1">
                  {strategyAllocations.map((s) => {
                    const isHovered = hoveredSliceId === s.id;
                    const barWidth = Math.max(1.5, Math.min(100, s.share));

                    return (
                      <div
                        key={s.id}
                        onMouseEnter={() => setHoveredSliceId(s.id)}
                        onMouseLeave={() => setHoveredSliceId(null)}
                        className={`space-y-1 p-1.5 rounded-lg transition cursor-pointer ${
                          isHovered ? "bg-surface-raised/70" : "hover:bg-surface-raised/40"
                        }`}
                      >
                        <div className="flex items-center justify-between gap-3 text-xs font-mono">
                          <span className="font-bold text-slate-900 truncate flex items-center gap-2 min-w-0">
                            <span
                              className="w-2.5 h-2.5 rounded-full shrink-0"
                              style={{ backgroundColor: s.color }}
                            />
                            {getStrategyIcon(s.id)}
                            <span className="truncate">{s.label}</span>
                          </span>
                          <span className="tabular-nums text-slate-700 whitespace-nowrap">
                            {usdPlain(s.notionalUsd)}{" "}
                            <span className="text-muted">({s.share.toFixed(1)}%)</span>
                            <span className="text-muted"> · {s.count} open</span>
                          </span>
                        </div>
                        <div className="h-1.5 w-full bg-slate-200 rounded-full overflow-hidden">
                          <div
                            className="h-full transition-all duration-300"
                            style={{ width: `${barWidth}%`, backgroundColor: s.color }}
                          />
                        </div>
                        {Math.abs(s.unrealizedUsd) > 0.005 && (
                          <div className="flex items-center justify-between text-[11px] font-mono">
                            <span className="text-slate-500">Unrealized P&L:</span>
                            <span className={`font-semibold ${pnlTone(s.unrealizedUsd)}`}>
                              {usd(s.unrealizedUsd)}
                            </span>
                          </div>
                        )}
                      </div>
                    );
                  })}
                  <div className="flex items-center justify-between pt-1 text-[11px] font-mono text-slate-500 border-t border-surface-border">
                    <span>
                      Total {usdPlain(totalStrategyDeployed)} deployed across {strategyAllocations.length} strateg{strategyAllocations.length === 1 ? "y" : "ies"}
                    </span>
                    <span>{openPositions.length} open position{openPositions.length === 1 ? "" : "s"}</span>
                  </div>
                </div>
              </div>
            ) : (
              <EmptyState
                title="No Active Positions"
                hint="No capital is currently deployed in open positions across any strategy."
              />
            )
          )}
        </div>

        <div className="bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
          <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold border-b border-surface-border pb-2.5 flex items-center space-x-2">
            <TrendingUp className="w-5 h-5 text-accent-strong" />
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

      {/* 3b. External positions: manual holdings closed via bot.
          Live account reads minus system legs; reduce-only closes with depth
          pricing. Placed after capital: same money-movement story. */}
      <ExternalPositionsGadget />

      {/* 3c. Traded opportunities: Net Profit + every dispatched signal with
          its atomic order trail (click row → detail popup). */}
      <TradedOpportunitiesGadget />

      {/* 3d. P&L/equity through time: realized P&L by strategy family plus
          external account flows, and the whole-account net equity walk. */}
      <NetPnlChartGadget />

      {/* 4. Risk detail: DeltaGuard neutrality + margin + breaker controls */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-4">
        {/* Left 2 Cols: DeltaGuard Net Position Neutrality Table */}
        <div className="xl:col-span-2 bg-surface rounded-xl border border-surface-border p-4 shadow-card space-y-3">
          <div className="flex items-center justify-between border-b border-surface-border pb-2.5">
            <div>
              <h2 className="text-[13px] font-mono tracking-wider text-slate-900 font-bold flex items-center space-x-2">
                <ShieldCheck className="w-5 h-5 text-accent-strong" />
                <span>DeltaGuard Cross-Exchange Signed Net Exposure</span>
              </h2>
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
                    const overLimit = Math.abs(delta.net_exposure_usd) > delta.delta_limit_usd;
                    const isFlat = Math.abs(delta.net_quantity) <= 1e-8;
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
                            {Number.isFinite(delta.net_exposure_usd) ? `$${delta.net_exposure_usd.toFixed(2)}` : "—"}
                          </span>
                        </td>
                        <td className="py-2.5 px-3 text-slate-500">${delta.delta_limit_usd.toLocaleString("en-US")} max</td>
                        <td className="py-2.5 px-3 text-right whitespace-nowrap">
                          {overLimit ? (
                            <span className="px-2.5 py-0.5 rounded-full text-xs font-bold bg-loss-dim text-loss border border-loss-border whitespace-nowrap inline-block">
                              Exposure Skew
                            </span>
                          ) : isFlat ? (
                            <span className="px-2.5 py-0.5 rounded-full text-xs font-bold bg-profit-dim text-profit border border-profit-border whitespace-nowrap inline-block">
                              Delta Neutral
                            </span>
                          ) : (
                            <span className="px-2.5 py-0.5 rounded-full text-xs font-bold bg-warn-dim text-warn-strong border border-warn-border whitespace-nowrap inline-block">
                              Residual
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
                {!margin ? (
                  <span className="font-bold text-muted whitespace-nowrap">—</span>
                ) : margin.status === "FLAT" ? (
                  <span className="font-bold text-slate-500 whitespace-nowrap">flat — no risk</span>
                ) : (
                  <span className="font-bold text-profit whitespace-nowrap">+{margin.liquidation_distance_pct.toFixed(1)}% safe</span>
                )}
              </div>
              <div className="h-2.5 w-full bg-slate-200 rounded-full overflow-hidden">
                <div
                  className="h-full bg-profit transition-all duration-300"
                  style={{ width: `${!margin ? 0 : margin.status === "FLAT" ? 100 : Math.min(100, Math.max(0, margin.liquidation_distance_pct))}%` }}
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
                  style={{ width: `${Math.min(100, Math.max(0, ((riskData?.consecutive_failures ?? 0) / (riskData?.max_failures_allowed ?? 3)) * 100))}%` }}
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
                className="w-full px-2 py-2 bg-warn-dim hover:bg-amber-100 text-warn-strong border border-warn-border rounded-lg text-xs font-mono font-bold flex items-center justify-center transition shadow-xs cursor-pointer"
                title="Reset Circuit Breaker — Resume Trading"
              >
                <Ban className="w-3.5 h-3.5 mr-1.5" />
                <span>Reset Circuit Breaker</span>
              </button>
            ) : (
              <button
                onClick={() => setShowHaltConfirm(true)}
                className="btn-danger w-full px-2 py-2 text-xs font-mono font-bold flex items-center justify-center shadow-xs cursor-pointer"
                title="Halt Trading — Trip the Circuit Breaker"
              >
                <Ban className="w-3.5 h-3.5 mr-1.5" />
                <span>Halt Trading</span>
              </button>
            )}
            <button
              onClick={async () => {
                setReconciling(true);
                try {
                  const r = await api.reconcileOrdersNow();
                  setReconcileNote(
                    `${r.polled} polled · ${r.resolved} resolved · ${r.escalated} escalated`,
                  );
                } catch {
                  setReconcileNote("reconciliation pass failed");
                } finally {
                  setReconciling(false);
                }
              }}
              disabled={reconciling}
              className="w-full px-2 py-2 bg-surface-alt hover:bg-surface-dim text-slate-600 border border-surface-border rounded-lg text-xs font-mono font-bold flex items-center justify-center transition shadow-xs cursor-pointer disabled:opacity-50"
              title="Poll every open venue order now and update fill state — runs automatically every 5s"
            >
              <span>{reconciling ? "Reconciling…" : "Reconcile Orders Now"}</span>
            </button>
            {reconcileNote && (
              <div className="text-[10px] font-mono text-slate-500 text-center">{reconcileNote}</div>
            )}
          </div>
        </div>
      </div>

      {/* 5. Per-Exchange Stats (per venue, existing IPC only) */}
      <ExchangeVenueGadget rows={exchangeRows} />



      {/* 7. Rebalance friction (last): venue-to-venue cost estimate.
          Pure frontend estimate over researched fee rails (no IPC); kept at
          the bottom so core monitor flow stays first. */}
      <RebalanceEstimatorGadget venueBalances={venueBalances} />

      <ConfirmActionModal
        spec={
          showHaltConfirm
            ? {
                title: "Halt Trading",
                subtitle: "Trip the risk circuit breaker",
                body: "This trips the circuit breaker and halts every new execution and coordination pass across all strategies. Open positions and resting venue orders are untouched — flatten explicitly if needed.",
                confirmLabel: "Halt Trading",
              }
            : null
        }
        onConfirm={onTriggerKillswitch}
        onCancel={() => setShowHaltConfirm(false)}
        onError={(msg) => onCommandError?.(msg)}
      />
    </div>
  );
};

export const PortfolioMonitor: React.FC<PortfolioMonitorProps> = React.memo(PortfolioMonitorInner);
